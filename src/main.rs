mod http;
mod lua_config;
mod lua_module;
mod resolve;

use crate::lua_config::{
    Cacheability, Client, ClientHandle, Context, ContextHandle, LuaConf, OnAcceptDenyResp,
    OnAcceptExpectHttpResp, OnAcceptRedirectResp, OnRequestExpectHttpBodyResp, Proxy,
    RequestHandle,
};
use crate::lua_module::ProxyModuleTable;
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
use std::os::fd::AsRawFd;
use std::rc::Rc;
use std::time::Duration;
use tealr::TypeWalker;

pub const CONF_POOL_SIZE: usize = 8;

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
        p.borrow_mut()
            .get_mut(backend_addr)
            .and_then(|v| v.pop_back())
    });
    let stream = if let Some(stream) = s
        && is_stream_alive(&stream)
    {
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
        .process_type::<Cacheability>()
        .process_type::<ClientHandle>()
        .process_type::<RequestHandle>()
        .process_type::<ContextHandle>()
        .process_type::<OnAcceptExpectHttpResp>()
        .process_type::<OnAcceptDenyResp>()
        .process_type::<OnAcceptRedirectResp>()
        .process_type::<OnRequestExpectHttpBodyResp>();

    let json = types.to_json().unwrap();
    if fs::write("conduit.json", json).is_err() {
        println!("Teal doc json write failed");
    }

    let conf_source = fs::read("config.lua").unwrap();

    let module_table = Rc::new(ProxyModuleTable::new().unwrap());
    let state = Rc::new(LuaConf::new(CONF_POOL_SIZE).unwrap());

    let source = state.lua.load(str::from_utf8(&conf_source).unwrap());
    source.exec().unwrap();

    let host: String = state
        .lua
        .globals()
        .get::<LuaAnyUserData>("proxy")
        .unwrap()
        .borrow::<Proxy>()
        .unwrap()
        .listen_addr
        .as_ref()
        .into();
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
    state: Rc<LuaConf>,
    module_table: Rc<ProxyModuleTable>,
) -> io::Result<()> {
    let buf = vec![0u8; 8 * 1024];
    let mut hdr_buf: [MaybeUninit<(Range<usize>, Range<usize>)>; 64] =
        [const { MaybeUninit::uninit() }; 64];

    // request
    let (buf, head, req_line, read_len) =
        http::read_headers(buf, &mut client, &mut hdr_buf[..]).await?;

    let addr = client.local_addr().unwrap();
    let client_t = Client::new(addr.ip(), addr.port());
    let context = Context::new();

    let (host, req) = http::parse_host_requset(&buf, &hdr_buf, &head, &req_line);
    let (backend_addr, header_add_vec) =
        resolve::resolve_backend(host, client_t, req, context, &state, &module_table).await?;

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
    let (buf, head, _req_line, read_len) =
        http::read_headers(buf, &mut backend, &mut hdr_buf).await?;

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

// i think there is a problem here
// we will lose our buffer on error
async fn fwd(buf: Vec<u8>, dst: &mut TcpStream, n: usize, offset: usize) -> io::Result<Vec<u8>> {
    let (res, view) = dst.write_all(buf.slice(offset..(offset + n))).await;
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
