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
    let mut buf = vec![0u8; 8 * 1024];
    let mut hdr_buf: [MaybeUninit<(Range<usize>, Range<usize>)>; MAX_HEADERS] =
        [const { MaybeUninit::uninit() }; MAX_HEADERS];
    let client_t = Client::new(peer_addr.ip(), peer_addr.port());

    // one client connection can carry many requests when kept alive; each
    // iteration is one full request/response exchange, reusing buf and hdr_buf
    loop {
        // read client request head
        let (b, head, req_line, read_len) = http::read_headers(buf, &mut client, &mut hdr_buf[..]).await?;
        buf = b;

        let context = Context::new();

        // build host + Request view
        let (host, req) = http::host_and_request(&buf, &hdr_buf, &head, &req_line);
        let keep_alive_requested = http::client_wants_keep_alive(&buf, &req_line, &req);

        // decide upstream
        let (backend_addr, header_add_vec) =
            hooks::resolve_backend(host, client_t.clone(), req, context, &state, &module_table).await?;

        // check out upstream connection
        let mut backend = net::get_conn(&backend_addr).await?;

        // relay request head + body
        let in_buf = read_len.saturating_sub(head.header_offset);
        buf = if let Some(n) = head.content_len {
            let b = fwd(buf, &mut backend, head.header_offset + in_buf.min(n), 0).await?;
            send_n(b, &mut client, &mut backend, n.saturating_sub(in_buf)).await?
        } else if head.chunked {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "we don't handle chunked yet",
            ));
        } else {
            write_request_head(buf, &mut backend, &head, header_add_vec).await?
        };

        // read upstream response head
        let (b, head, _req_line, read_len) = http::read_headers(buf, &mut backend, &mut hdr_buf).await?;
        buf = b;

        // a response with no Content-Length (and not chunked) has no reliable
        // end-of-body signal short of closing the connection, so keep-alive is
        // only possible when the upstream gave us an exact length
        let keep_alive = keep_alive_requested && head.content_len.is_some();

        // relay response head + body
        if let Some(n) = head.content_len {
            let b = write_response_head(buf, &mut client, &head, &hdr_buf, keep_alive).await?;

            // the initial read from backend may have already captured some (or
            // all) of the body alongside the headers; forward that portion
            // before pulling any remaining bytes straight from the backend
            let in_buf = read_len.saturating_sub(head.header_offset);
            let b = fwd(b, &mut client, in_buf.min(n), head.header_offset).await?;
            buf = send_n(b, &mut backend, &mut client, n.saturating_sub(in_buf)).await?;
        } else if head.chunked {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "we don't handle chunked yet",
            ));
        } else {
            buf = fwd(buf, &mut client, head.header_offset, 0).await?;
        }

        // return connection to the pool
        net::put_conn(backend_addr, backend).await;

        if !keep_alive {
            return Ok(());
        }
    }
}
