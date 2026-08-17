mod lua_config;
mod lua_module;

use crate::lua_config::{Cacheability, CachedValue, Client, ClientHandle, Context, ContextHandle, HeadersPatchBlock, Hook, HookCacheKeyKind, LuaState, OnAcceptDenyResp, OnAcceptExpectHttpResp, OnAcceptFn, OnAcceptRedirectResp, OnAcceptResp, OnRequestExpectHttpBodyResp, OnRequestResp, Proxy, Request, RequestHandle};
use crate::lua_module::{HookResult, ProxyModule, ProxyModuleTable};
use mlua::prelude::*;
use monoio::buf::{IoBuf, IoBufMut};
use monoio::io::{AsyncReadRent, AsyncWriteRent, AsyncWriteRentExt};
use monoio::net::{TcpListener, TcpStream};
use tealr::TypeWalker;
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io;
use std::mem::MaybeUninit;
use std::ops::Range;
use std::os::fd::AsRawFd;
use std::rc::Rc;
use std::time::{Duration, Instant};


pub const CONF_POOL_SIZE: usize = 8;

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

const MAX_IDLE: usize = 32;

thread_local! {
    static CONN_POOL: RefCell<HashMap<String, VecDeque<TcpStream>>> = RefCell::new(HashMap::new());
}

fn is_stream_alive(stream: &TcpStream) -> bool {
    let fd = stream.as_raw_fd();
    let mut buf = [0u8; 1];

    // MSG_PEEK: inspect buffer without pulling data out.
    // MSG_DONTWAIT: instantly return if no data exists instead of waiting.
    let res = unsafe {
        libc::recv(
            fd,
            buf.as_mut_ptr() as *mut libc::c_void,
            1,
            libc::MSG_PEEK | libc::MSG_DONTWAIT,
        )
    };

    if res > 0 {
        true
    } else if res == 0 {
        false
    } else {
        let err = io::Error::last_os_error();
        matches!(err.kind(), io::ErrorKind::WouldBlock)
    }
}

async fn get_conn(backend_addr: &String) -> io::Result<TcpStream> {
    let s = CONN_POOL.with(|p| {
        p.borrow_mut().get_mut(backend_addr).and_then(|v| v.pop_back())
    });
    let stream = if let Some(stream) = s && is_stream_alive(&stream){
           stream
    } else {
        let s = TcpStream::connect(backend_addr).await?;
        let _ = s.set_nodelay(true);
        s
    };
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
            let bytes = s.as_bytes();
            Ok(LuaValue::String(dest_lua.create_string(bytes)?))
        }

        LuaValue::Table(t) => {
            let dest_table = dest_lua.create_table()?;
            for pair in t.pairs::<LuaValue, LuaValue>() {
                let (k, v) = pair?;
                let new_k = transfer_value_between_states(k, dest_lua)?;
                let new_v = transfer_value_between_states(v, dest_lua)?;
                dest_table.set(new_k, new_v)?;
            }
            Ok(LuaValue::Table(dest_table))
        }

        _ => Err(mlua::Error::RuntimeError(
            "Cannot move functions, threads, or raw userdata pointers between independent states".into()
        )),
    }
}

async fn run_hooks(
    host: &str,
    client: &Client,
    req: &Request,
    mut context: Context,
    state: &LuaState,
    module_table: &ProxyModuleTable
) {
    let hooks = &state.hooks;
    let n = hooks.borrow().get(host).map(|v| v.borrow().len()).unwrap_or(0);
    for i in 0..n {
        let (module, cache_ttl, key) = hook_at(host, i, client, req, state, module_table);

        let cached: Option<HookResult> = if let Some(k) = &key {
            hooks.borrow().get(host).unwrap().borrow()[i].cached_result.get(k.as_ref()).and_then(|c| c.get().cloned())
        } else {
            None
        };
        let resp: Option<HookResult> = if cached.is_some() {
            cached
        } else {
            let f = module.hook_handler.borrow().as_ref().cloned();
            if let Some(f) = f {
                let h = module.pool.checkout();
                h.set_client(client.clone());
                h.set_req(req.clone());

                // yields here if the module awaits (e.g. conduit.sleep), letting
                // other requests on this thread run. `h` is held across the await,
                // so this request keeps its own pool entry while suspended.
                let r = f.call_async::<HookResult>((&h.client_ud, &h.req_ud)).await.unwrap();

                if let (Some(t), Some(k)) = (cache_ttl, key) {
                    let map = hooks.borrow();
                    let mut hv = map.get(host).unwrap().borrow_mut();
                    hv[i].cached_result.insert(k.into_owned(), CachedValue::new(r.clone(), t));
                }
                Some(r)
            } else {
                None
            }
        };
        if let Some(HookResult::ContinueWithCtx(update_ctx)) = resp {
            for (key, val) in update_ctx.updates {
                if let Ok(v) = transfer_value_between_states(val, &state.lua) {
                    context.insert(key, v);
                }
            }
        }
    }
}


fn hook_at<'a>(
    host: &str,
    idx: usize,
    client: &'a Client,
    req: &'a Request,
    state: &LuaState,
    module_table: &ProxyModuleTable
) -> (Rc<ProxyModule>, Option<Duration>, Option<Cow<'a, str>>) {
    let map = state.hooks.borrow();
    let hv = map.get(host).unwrap().borrow();
    let h = &hv[idx];

    let (cache_ttl, key) = if let Some(ref c) = h.cacheability {
        (
            Some(c.ttl),
            Some(match &c.key {
                HookCacheKeyKind::Url => Cow::Borrowed(req.0.path()),
                HookCacheKeyKind::Header(hh) => Cow::Borrowed(req.0.header(hh).unwrap_or("")),
                HookCacheKeyKind::Ip => Cow::Owned(client.0.ip.to_string()),
                HookCacheKeyKind::Custom(c) => Cow::Owned(c.clone()),
            })
        )
    } else {
        (None, None)
    };

    let module = module_table.modules.get(h.name.as_str()).unwrap().clone();
    (module, cache_ttl, key)
}

async fn resolve_backend(
    host: &str,
    client: Client,
    req: Request,
    context: Context,
    state: &LuaState,
    module_table: &ProxyModuleTable,
) -> io::Result<(String, Option<Vec<(String, String)>>)> {
    run_hooks(host, &client, &req, context.clone(), state, module_table).await;

    let handler_fn = state.req_handlers.borrow();
    let lua_fn = handler_fn.get(host).cloned().unwrap();

    let call_lua_fn = || {
        let h = state.pool.checkout();
        h.set_client(client.clone());
        h.set_req(req.clone());
        h.set_ctx(context.clone());

        lua_fn
            .call::<OnRequestResp>((&h.client_ud, &h.req_ud, &h.ctx_ud))
            .unwrap()
    };

    let handle_resp = |resp: OnRequestResp| {
        if let OnRequestResp::OnRequestExpectHttpBodyResp(b) = resp {
            Ok((b.upstream, b.headers_patch.map(|p| p.add_headers)))
        } else {
            Err(io::Error::new(io::ErrorKind::InvalidData, "unexpected response type"))
        }
    };

    state.lua.set_hook(
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

    let resp = call_lua_fn();
    state.lua.remove_hook();
    handle_resp(resp)
}

fn enable_defer_accept(listener: &TcpListener, timeout: Duration) -> io::Result<()> {
    let fd = listener.as_raw_fd();
    let seconds = timeout.as_secs() as libc::c_int;

    unsafe {
        let ret = libc::setsockopt(
            fd,
            libc::IPPROTO_TCP,
            libc::TCP_DEFER_ACCEPT,
            &seconds as *const libc::c_int as *const libc::c_void,
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        );

        if ret == -1 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

#[allow(unused_variables)]
#[monoio::main(threads = 8, timer_enabled = true)]
async fn main() {
    let types = TypeWalker::new()
        .process_type::<Proxy>()
        .process_type::<ClientHandle>()
        .process_type::<RequestHandle>()
        .process_type::<ContextHandle>()
        .process_type::<OnAcceptExpectHttpResp>()
        .process_type::<OnAcceptDenyResp>()
        .process_type::<OnAcceptRedirectResp>()
        .process_type::<OnRequestExpectHttpBodyResp>();

    let json = types.to_json().unwrap();
    fs::write("conduit.json", json);

    let conf_source = fs::read("config.lua").unwrap();

    let module_table = Rc::new(ProxyModuleTable::new());
    let state = Rc::new(LuaState::new(CONF_POOL_SIZE).unwrap());

    // reusing handles means lua allocates so little that the default incremental
    // GC stops keeping up; generational collects it at no measurable cost
    state.lua.gc_gen(20, 100);

    let source = state.lua.load(str::from_utf8(&conf_source).unwrap());
    source.exec().unwrap();

    let host: String = state.lua.globals().get::<LuaAnyUserData>("proxy").unwrap().borrow::<Proxy>().unwrap().listen_addr.as_ref().into();
    println!("{}", host);
    let listener = TcpListener::bind(host).unwrap();
    enable_defer_accept(&listener, Duration::from_secs(1)).unwrap();

    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        let state_pass = state.clone();
        let module_table_pass = module_table.clone();
        monoio::spawn(async move {
            let _r = forward_http(stream, state_pass, module_table_pass).await;
        });
    }
}

async fn forward_http(
    mut client: TcpStream,
    state: Rc<LuaState>,
    module_table: Rc<ProxyModuleTable>
) -> io::Result<()> {
    let buf = vec![0u8; 8 * 1024];
    let mut hdr_buf: [MaybeUninit<(Range<usize>, Range<usize>)>; 64] =
        [const { MaybeUninit::uninit() }; 64];

    // request
    let (buf, head, req_line, read_len) = read_headers(buf, &mut client, &mut hdr_buf[..]).await?;

    let addr = client.local_addr().unwrap();
    let client_t = Client::new(addr.ip(), addr.port());
    let context = Context::new();

    let (host, req) = get_host_requset(&buf, &hdr_buf, &head, &req_line);
    let (backend_addr, header_add_vec) =
        resolve_backend(host, client_t, req, context, &state, &module_table).await?;

    let mut backend = get_conn(&backend_addr).await?;

    let in_buf = read_len.saturating_sub(head.header_offset);
    let buf = if let Some(n) = head.content_len {
        let buf = fwd(buf, &mut backend, head.header_offset + in_buf.min(n), 0).await?;
        send_n(buf, &mut client, &mut backend, n.saturating_sub(in_buf)).await?
    } else if head.chunked {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "we don't handle chunked yet",
        ));
    } else {
        //let buf = fwd(buf, &mut backend, head.start_of_header, 0).await?;
        let mut temp_buf: Vec<u8> = Vec::with_capacity(256 + head.header_offset);
        temp_buf.extend_from_slice(&buf[0..head.start_of_header]);
        if let Some(headers) = header_add_vec {
            for (k, v) in headers {
                temp_buf.extend_from_slice(k.as_bytes());
                temp_buf.extend_from_slice(b": ");
                temp_buf.extend_from_slice(v.as_bytes());
                temp_buf.extend_from_slice(b"\r\n");
            }
        }
        temp_buf.extend_from_slice(&buf[head.start_of_header..head.header_offset]);
        let (res, _) = backend.write_all(temp_buf).await;
        res?;
        buf
    };

    // response
    let (buf, head, _req_line, read_len) = read_headers(buf, &mut backend, &mut hdr_buf).await?;

    if let Some(n) = head.content_len {
        let mut temp_buf: Vec<u8> = Vec::with_capacity(256 + read_len);
        temp_buf.extend_from_slice(&buf[0..head.start_of_header]);
        temp_buf.extend_from_slice(b"Connection: Close\r\n");
        temp_buf.extend_from_slice(&buf[head.start_of_header..]);
        let (res, _) = client.write_all(temp_buf).await;
        res?;

        let in_buf = read_len.saturating_sub(head.header_offset);
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
) -> (&'a str, Request) {
    let n = head.header_count.min(64);
    let mut headers = Vec::with_capacity(n);
    for i in 0..n {
        headers.push(unsafe { hdr[i].assume_init_read() });
    }
    let host =
        unsafe { std::str::from_utf8_unchecked(&buf[req_line.host.as_ref().unwrap().clone()]) };
    (
        host,
        Request::new(
            &buf[..head.header_offset],
            req_line.method.as_ref().unwrap().clone(),
            req_line.path.as_ref().unwrap().clone(),
            headers,
        ),
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
