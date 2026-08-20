use crate::http::{self, MAX_HEADERS};
use crate::hooks;
use crate::lua_config::{Client, Context, LuaState};
use crate::lua_module::ProxyModuleTable;
use crate::net;
use crate::relay::{fwd, send_n, write_request_head, write_response_head};
use monoio::net::TcpStream;
use std::io;
use std::mem::MaybeUninit;
use std::net::SocketAddr;
use std::ops::Range;
use std::rc::Rc;

pub async fn forward_http(
    mut client: TcpStream,
    peer_addr: SocketAddr,
    state: Rc<LuaState>,
    module_table: Rc<ProxyModuleTable>
) -> io::Result<()> {
    let buf = vec![0u8; 8 * 1024];
    let mut hdr_buf: [MaybeUninit<(Range<usize>, Range<usize>)>; MAX_HEADERS] =
        [const { MaybeUninit::uninit() }; MAX_HEADERS];

    // read client request head
    let (buf, head, req_line, read_len) = http::read_headers(buf, &mut client, &mut hdr_buf[..]).await?;

    let client_t = Client::new(peer_addr.ip(), peer_addr.port());
    let context = Context::new();

    // build host + Request view
    let (host, req) = http::host_and_request(&buf, &hdr_buf, &head, &req_line);

    // decide upstream
    let (backend_addr, header_add_vec) =
        hooks::resolve_backend(host, client_t, req, context, &state, &module_table).await?;

    // check out upstream connection
    let mut backend = net::get_conn(&backend_addr).await?;

    // relay request head + body
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
        write_request_head(buf, &mut backend, &head, header_add_vec).await?
    };

    // read upstream response head
    let (buf, head, _req_line, read_len) = http::read_headers(buf, &mut backend, &mut hdr_buf).await?;

    // relay response head + body
    if let Some(n) = head.content_len {
        let buf = write_response_head(buf, &mut client, &head, read_len).await?;

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

    // return connection to the pool
    net::put_conn(backend_addr, backend).await;
    Ok(())
}
