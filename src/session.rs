use crate::http::{self, MAX_HEADERS};
use crate::lua_config::{Client, LuaConf};
use crate::net;
use crate::relay::{fwd, relay_chunked, send_n, write_request_head, write_response_head};
use crate::resolve;
use monoio::net::TcpStream;
use std::io;
use std::mem::MaybeUninit;
use std::net::SocketAddr;
use std::ops::Range;
use std::rc::Rc;

pub async fn forward_http(
    mut client: TcpStream,
    peer_addr: SocketAddr,
    state: Rc<LuaConf>,
) -> io::Result<()> {
    let mut buf = vec![0u8; 8 * 1024];
    let mut hdr_buf: [MaybeUninit<(Range<usize>, Range<usize>)>; MAX_HEADERS] =
        [const { MaybeUninit::uninit() }; MAX_HEADERS];
    let client_t = Client::new(peer_addr.ip(), peer_addr.port());

    if !resolve::accept_allowed(&state, &client_t)? {
        return Ok(());
    }

    // one client connection can carry many requests when kept alive; each
    // iteration is one full request/response exchange, reusing buf and hdr_buf
    loop {
        // read client request head
        let (b, head, req_line, read_len) =
            http::read_headers(buf, &mut client, &mut hdr_buf[..]).await?;
        buf = b;

        // build host + Request view
        let (host, req) = http::host_and_request(&buf, &hdr_buf, &head, &req_line);
        let keep_alive_requested = http::client_wants_keep_alive(&buf, &req_line, &req);

        // decide upstream
        let (backend_addr, header_add_vec) =
            resolve::resolve_backend(host, client_t.clone(), req, &state)
                .await?;

        // check out upstream connection
        let mut backend = net::get_conn(&backend_addr).await?;

        // relay request head + body
        let in_buf = read_len.saturating_sub(head.header_offset);
        buf = if let Some(n) = head.content_len {
            let b = write_request_head(buf, &mut backend, &head, header_add_vec, in_buf.min(n)).await?;
            send_n(b, &mut client, &mut backend, n.saturating_sub(in_buf)).await?
        } else if head.chunked {
            // relay_chunked starts at header_offset, so the head is written alone here
            let b = write_request_head(buf, &mut backend, &head, header_add_vec, 0).await?;
            relay_chunked(b, head.header_offset, read_len, &mut client, &mut backend).await?
        } else {
            write_request_head(buf, &mut backend, &head, header_add_vec, 0).await?
        };

        // read upstream response head
        let (b, head, _req_line, read_len) =
            http::read_headers(buf, &mut backend, &mut hdr_buf).await?;
        buf = b;

        // a response with no Content-Length and no chunked framing has no
        // reliable end-of-body signal short of closing the connection, so
        // keep-alive needs one or the other
        let keep_alive = state.keep_alive.get()
            && keep_alive_requested
            && (head.content_len.is_some() || head.chunked);

        // relay response head + body
        if let Some(n) = head.content_len {
            // the initial read from backend may have already captured some (or
            // all) of the body alongside the headers; it goes out with the head
            let in_buf = read_len.saturating_sub(head.header_offset);
            let b = write_response_head(
                buf, &mut client, &head, &hdr_buf, keep_alive, in_buf.min(n),
            )
            .await?;
            buf = send_n(b, &mut backend, &mut client, n.saturating_sub(in_buf)).await?;
        } else if head.chunked {
            let b =
                write_response_head(buf, &mut client, &head, &hdr_buf, keep_alive, 0).await?;
            buf = relay_chunked(b, head.header_offset, read_len, &mut backend, &mut client).await?;
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
