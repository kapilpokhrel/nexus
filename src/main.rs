mod lua_config;
mod lua_module;

use crate::lua_config::{Client, Context, HeadersPatchBlock, OnRequestResp, Proxy, Request};
use crate::lua_module::{HookResult, ProxyModuleTable};
use mlua::prelude::*;
use monoio::buf::{IoBuf, IoBufMut};
use monoio::io::{AsyncReadRent, AsyncWriteRentExt};
use monoio::net::{TcpListener, TcpStream};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io;
use std::mem::MaybeUninit;
use std::ops::Range;
use std::rc::Rc;
use std::time::{Duration, Instant};


#[derive(Debug)]
struct HttpHead {
    content_len: Option<usize>,
    chunked: bool,
    start_of_header: usize,
    header_count: usize,
    header_offset: usize,
}

#[derive(Debug)]
struct RequestInfo {
    host: Option<Range<usize>>,
    method: Option<Range<usize>>,
    path: Option<Range<usize>>,
}

const MAX_IDLE: usize = 15;

thread_local! {
    static CONN_POOL: RefCell<HashMap<String, VecDeque<TcpStream>>> = RefCell::new(HashMap::new());
    static LUA_FN_CACHE: RefCell<HashMap<String, (OnRequestResp, Instant)>> = RefCell::new(HashMap::new());
}

async fn get_conn(backend_addr: &String) -> io::Result<TcpStream> {
    let s = CONN_POOL.with(|p| {
        p.borrow_mut().get_mut(backend_addr).and_then(|v| v.pop_back())
    });
    let stream = match s {
        Some(stream) => {
           stream
        }
        None => TcpStream::connect(backend_addr).await?
    };
    let _ = stream.set_nodelay(true);
    Ok(stream)
}

async fn put_conn(backend_addr: String, s: TcpStream) {
    CONN_POOL.with_borrow_mut(|h| {
        let vq = h.entry(backend_addr).or_insert(VecDeque::new());
        if vq.len() < MAX_IDLE {
            vq.push_back(s)
        }
    });
}


/// Deep-copies an arbitrary Lua value from its original state into a completely different Lua state.
fn transfer_value_between_states(
    val: LuaValue,
    dest_lua: &Lua
) -> mlua::Result<LuaValue> {
    match val {
        LuaValue::Nil => Ok(LuaValue::Nil),
        LuaValue::Boolean(b) => Ok(LuaValue::Boolean(b)),
        LuaValue::Integer(i) => Ok(LuaValue::Integer(i)),
        LuaValue::Number(n) => Ok(LuaValue::Number(n)),

        LuaValue::String(s) => {
            // Extracts raw bytes without UTF-8 re-validation overhead
            let bytes = s.as_bytes();
            Ok(LuaValue::String(dest_lua.create_string(bytes)?))
        }

        LuaValue::Table(t) => {
            let dest_table = dest_lua.create_table()?;
            // Iterate over all arbitrary keys and LuaValues in the source table
            for pair in t.pairs::<LuaValue, LuaValue>() {
                let (k, v) = pair?;
                let new_k = transfer_value_between_states(k, dest_lua)?;
                let new_v = transfer_value_between_states(v, dest_lua)?;
                dest_table.set(new_k, new_v)?;
            }
            Ok(LuaValue::Table(dest_table))
        }

        // Safety Fallback: Refuse to move state-bound system components
        _ => Err(mlua::Error::RuntimeError(
            "Cannot move functions, threads, or raw userdata pointers between independent states".into()
        )),
    }
}

async fn resolve_backend<'a>(
    host: &'a str,
    client: Client,
    req: Request<'a>,
    mut context: Context,
    lua: Lua,
    handlers: Rc<RefCell<HashMap<Box<str>, (LuaFunction, Option<Duration>)>>>,
    module_table: Rc<ProxyModuleTable>,
) -> io::Result<(String, Option<Vec<(String, String)>>)> {

    for (_, module) in module_table.modules.iter() {
        if let Some(f) = module.hook_handlers.borrow().get(host) {
            let resp = module.lua.scope(|scope| {
                let req = scope.create_userdata(&req).unwrap();
                let client = scope.create_userdata(&client).unwrap();
                let context_ud = scope.create_userdata(&context).unwrap();

                let resp = f.call::<HookResult>((&client, &req, context_ud)).unwrap();
                Ok(resp)
            }).unwrap();
            if let HookResult::ContinueWithCtx(update_ctx) = resp {
                for (key, val) in update_ctx.updates {
                    let v = transfer_value_between_states(val, &lua);
                    if let Ok(val) = v {
                        context.ctx.insert(key, val);
                    }
                }
            }
        }
    }

    let handler_fn = handlers.borrow();
    let (lua_fn, cache_ttl) = handler_fn.get(host).unwrap();

    let call_lua_fn = || {
        lua.set_hook(
            LuaHookTriggers {
                every_line: false,
                on_calls: false,
                on_returns: false,
                // Check every 1000 VM instructions
                every_nth_instruction: Some(1000),
            },
            move |_, _| {
                // error after 1000 VM instructions
                Err(mlua::Error::RuntimeError("timeout exceeded".to_string()))
                // or we can check elapsed time and if not timeout, `Ok(LuaVmState::Continue)`
            },
        ).unwrap();

        let resp = lua.scope(|scope| {
            // benchmark shows that adding creating lua userdata takes significant time...
            let req = scope.create_userdata(&req).unwrap();
            let client = scope.create_userdata(&client).unwrap();
            let context_ud = scope.create_userdata(&context)?;

            lua_fn
                .call::<OnRequestResp>((client, req, context_ud))
        }).unwrap();
        lua.remove_hook();
        resp
    };

    let handle_resp = |resp: &OnRequestResp| {
        if let OnRequestResp::ExpectHttpBody(b) = resp {
            if let Some(ref p) = b.headers_patch {
                let mut add_headers: Vec<(String, String)> = Vec::with_capacity(p.add_headers.len());
                for (k, v) in p.add_headers.iter() {
                    add_headers.push((k.to_string(), v.to_string()));
                }
                Ok((b.upstream.to_string(), Some(add_headers)))
            } else {
                Ok((b.upstream.to_string(), None))
            }
        } else {
            Err(io::Error::new(io::ErrorKind::InvalidData, "unexpected response type"))
        }
    };

    if let Some(t) = cache_ttl {
        LUA_FN_CACHE.with_borrow_mut(|m| {
            if !m.contains_key(host) {
                m.insert(host.into(), (call_lua_fn(), Instant::now() + t.to_owned()));
            }
            let cached_value = m.get(host).unwrap();
            handle_resp(&cached_value.0)
        })
    } else {
        handle_resp(&call_lua_fn())
    }
}

#[allow(unused_variables)]
#[monoio::main(threads = 8)]
async fn main() {
    let conf_source = fs::read("config.lua").unwrap();
    // lua setup

    let lua = Lua::new();

    let module_table = Rc::new(ProxyModuleTable::new());

    let req_handlers: Rc<RefCell<HashMap<Box<str>,(LuaFunction, Option<Duration>)>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let proxy = Proxy {
        on_accept_fn: Rc::new(RefCell::new(None)),
        req_handlers: req_handlers.clone(),
        listen_addr: "".into(),
    };

    let proxy_ud = lua.create_userdata(proxy).unwrap();
    lua.globals().set("proxy", proxy_ud).unwrap();

    let source = lua.load(str::from_utf8(&conf_source).unwrap());
    source.exec().unwrap();

    let host: String = lua.globals().get::<LuaAnyUserData>("proxy").unwrap().borrow::<Proxy>().unwrap().listen_addr.as_ref().into();
    println!("{}", host);
    let listener = TcpListener::bind(host).unwrap();

    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        let lua_pass = lua.clone();
        let handlers_pass = req_handlers.clone();
        let module_table_pass = module_table.clone();
        monoio::spawn(async move {
            let _r = forward_http(stream, lua_pass, handlers_pass, module_table_pass).await;
        });
    }
}

async fn forward_http(
    mut client: TcpStream,
    lua: Lua,
    handlers: Rc<RefCell<HashMap<Box<str>, (LuaFunction, Option<Duration>)>>>,
    module_table: Rc<ProxyModuleTable>
) -> io::Result<()> {
    let buf = vec![0u8; 8 * 1024];
    let mut hdr_buf: [MaybeUninit<(Range<usize>, Range<usize>)>; 64] =
        [const { MaybeUninit::uninit() }; 64];

    // request
    let (buf, head, req_line, read_len) = read_headers(buf, &mut client, &mut hdr_buf[..]).await?;

    let client_t = Client {
        ip: client.local_addr().unwrap().ip().to_string(),
        port: client.local_addr().unwrap().port(),
    };
    let context = Context::new();

    let (host, req) = get_host_requset(&buf, &hdr_buf, &head, &req_line);
    let (backend_addr, header_add_vec) = resolve_backend(host, client_t, req, context, lua, handlers, module_table).await?;

    let mut backend = get_conn(&backend_addr).await?;

    let in_buf = read_len.saturating_sub(head.header_offset);
    let buf = if let Some(n) = head.content_len {
        let buf = fwd_or_reconnect(buf, &mut backend, head.header_offset + in_buf.min(n)).await?;
        send_n(buf, &mut client, &mut backend, n.saturating_sub(in_buf)).await?
    } else if head.chunked {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "we don't handle chunked yet",
        ));
    } else {
        let buf = fwd_or_reconnect(buf, &mut backend, head.start_of_header).await?;
        if let Some(headers) = header_add_vec {
            for (k, v) in headers {
                let line = format!("{k}: {v}\r\n").into_bytes();
                backend.write_all(line).await;
            }
        }
        fwd(buf, &mut backend, head.header_offset.saturating_sub(head.start_of_header), head.start_of_header).await?
    };

    // response
    let (buf, head, _req_line, read_len) = read_headers(buf, &mut backend, &mut hdr_buf).await?;

    let in_buf = read_len.saturating_sub(head.header_offset);
    if let Some(n) = head.content_len {
        let buf = fwd(buf, &mut client, head.start_of_header, 0).await?;
        client.write_all("Connection: Close\r\n").await;
        let buf = fwd(buf, &mut client, head.header_offset.saturating_sub(head.start_of_header), head.start_of_header).await?;
        let buf = fwd(buf, &mut client, in_buf.min(n), head.header_offset).await?;
        let _ = send_n(buf, &mut backend, &mut client, n.saturating_sub(in_buf)).await?;
    } else if head.chunked {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "we don't handle chunked yet",
        ));
    } else {
        let _ = fwd(buf, &mut client, head.header_offset, 0).await?;
    }

    put_conn(backend_addr, backend).await;
    Ok(())
}

fn range_of(buf: &[u8], sub: &[u8]) -> Range<usize> {
    let start = sub.as_ptr() as usize - buf.as_ptr() as usize;
    start..start + sub.len()
}

fn get_host_requset<'a>(
    buf: &'a [u8],
    hdr: &[MaybeUninit<(Range<usize>, Range<usize>)>; 64],
    head: &HttpHead,
    req_line: &RequestInfo,
) -> (&'a str, Request<'a>) {
    let mut hdrs = [("", ""); 64];
    for i in 0..head.header_count.min(64) {
        let (nr, vr) = unsafe { hdr[i].assume_init_read() };
        hdrs[i] = (unsafe { std::str::from_utf8_unchecked(&buf[nr]) }, unsafe {
            std::str::from_utf8_unchecked(&buf[vr])
        });
    }
    let host =
        unsafe { std::str::from_utf8_unchecked(&buf[req_line.host.as_ref().unwrap().clone()]) };
    let method =
        unsafe { std::str::from_utf8_unchecked(&buf[req_line.method.as_ref().unwrap().clone()]) };
    let path =
        unsafe { std::str::from_utf8_unchecked(&buf[req_line.path.as_ref().unwrap().clone()]) };
    (
        host,
        Request {
            path,
            method,
            headers: hdrs,
            headers_count: head.header_count,
        },
    )
}

async fn read_headers(
    mut buf: Vec<u8>,
    stream: &mut TcpStream,
    hdr_ranges: &mut [MaybeUninit<(Range<usize>, Range<usize>)>],
) -> io::Result<(Vec<u8>, HttpHead, RequestInfo, usize)> {
    let mut ptr = 0;
    loop {
        let (res, slice) = stream.read(buf.slice_mut(ptr..)).await;
        buf = slice.into_inner();
        let n = res?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "eof"));
        }
        ptr += n;

        let Some(se) = buf[..ptr].windows(2).position(|w| w == b"\r\n") else {
            if ptr == buf.len() {
                buf.resize(buf.len() * 2, 0);
            }
            continue;
        };
        let se = se + 2; // past the \r\n — start of headers

        let mut raw = [httparse::EMPTY_HEADER; 64];
        match httparse::parse_headers(&buf[se..ptr], &mut raw) {
            Ok(httparse::Status::Complete((hlen, parsed))) => {
                let header_end = se + hlen;
                for (i, h) in parsed.iter().enumerate() {
                    hdr_ranges[i] = MaybeUninit::new((
                        range_of(&buf, h.name.as_bytes()),
                        range_of(&buf, h.value),
                    ));
                }
                let cl = parsed
                    .iter()
                    .find(|h| h.name.eq_ignore_ascii_case("Content-Length"))
                    .and_then(|h| std::str::from_utf8(h.value).ok()?.trim().parse().ok());
                let chunked = parsed.iter().any(|h| {
                    h.name.eq_ignore_ascii_case("Transfer-Encoding")
                        && h.value
                            .split(|&b| b == b',')
                            .any(|v| v.trim_ascii().eq_ignore_ascii_case(b"chunked"))
                });
                let host = parsed
                    .iter()
                    .find(|h| h.name.eq_ignore_ascii_case("Host"))
                    .map(|h| range_of(&buf, h.value));
                let h_count = parsed.len();

                let (method, path) = {
                    let req = &buf[..se - 2];
                    let sp1 = req.iter().position(|&b| b == b' ');
                    let sp2 = sp1.and_then(|i| {
                        req[i + 1..]
                            .iter()
                            .position(|&b| b == b' ')
                            .map(|j| i + 1 + j)
                    });
                    match (sp1, sp2) {
                        (Some(e), Some(s)) if !req[..e].contains(&b'/') => {
                            (Some(0..e), Some(e + 1..s))
                        }
                        _ => (None, None),
                    }
                };

                return Ok((
                    buf,
                    HttpHead {
                        content_len: cl,
                        chunked,
                        start_of_header: se,
                        header_count: h_count,
                        header_offset: header_end,
                    },
                    RequestInfo { host, method, path },
                    ptr,
                ));
            }
            Ok(httparse::Status::Partial) => {
                if (se..ptr.saturating_sub(3))
                    .rev()
                    .any(|i| buf[i..i + 4] == b"\r\n\r\n"[..])
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "too many headers",
                    ));
                }
                if ptr == buf.len() {
                    buf.resize(buf.len() * 2, 0);
                }
                continue;
            }
            Err(e) => return Err(io::Error::new(io::ErrorKind::InvalidData, e)),
        }
    }
}

async fn fwd_or_reconnect(mut buf: Vec<u8>, dst: &mut TcpStream, n: usize) -> io::Result<Vec<u8>> {
    let (res, view) = dst.write_all(buf.slice(..n)).await;
    buf = view.into_inner();
    match res {
        Ok(_) => Ok(buf),
        Err(_) => {
            let addr = dst.local_addr()?;
            let new_dst = TcpStream::connect(addr).await?;
            let _old = std::mem::replace(dst, new_dst);
            let (res, view) = dst.write_all(buf.slice(..n)).await;
            res?;
            Ok(view.into_inner())
        }
    }
}

// i think there is a problem here
// we will lose our buffer on error
async fn fwd(buf: Vec<u8>, dst: &mut TcpStream, n: usize, offset: usize) -> io::Result<Vec<u8>> {
    let (res, view) = dst.write_all(buf.slice(offset..(offset+n))).await;
    res?;
    Ok(view.into_inner())
}

async fn send_n(
    mut buf: Vec<u8>,
    src: &mut TcpStream,
    dst: &mut TcpStream,
    n: usize,
) -> io::Result<Vec<u8>> {
    let mut rem = n;
    while rem > 0 {
        let sz = rem.min(buf.len());
        let (res, slice) = src.read(buf.slice_mut(..sz)).await;
        buf = slice.into_inner();
        let got = res?;
        if got == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "eof"));
        }
        let (res, view) = dst.write_all(buf.slice(..got)).await;
        res?;
        buf = view.into_inner();
        rem -= got;
    }
    Ok(buf)
}
