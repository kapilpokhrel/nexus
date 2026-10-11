use crate::http::{self, MAX_HEADERS};
use crate::lua_config::{Client, ForwardHttp, LuaConf, OnAcceptHandlerResp, OnRequestHandlerResp};
use crate::net;
use crate::relay::{relay_chunked, relay_until_eof, send_n, write_head};
use crate::resolve;
use monoio::io::AsyncWriteRentExt;
use monoio::net::TcpStream;
use std::io;
use std::mem::MaybeUninit;
use std::net::SocketAddr;
use std::ops::Range;
use std::rc::Rc;

#[derive(Clone, Copy)]
enum BodyFraming {
    None,
    ContentLength(usize),
    Chunked,
    UntilEof,
}

struct HttpMessage<'a> {
    buf: Vec<u8>,
    head: &'a http::HttpHead,
    hdr_ranges: &'a [MaybeUninit<(Range<usize>, Range<usize>)>],
    read_len: usize,
    framing: BodyFraming,
}

async fn relay_http_message(
    src: &mut TcpStream,
    dst: &mut TcpStream,
    message: HttpMessage<'_>,
    add_headers: &[(String, String)],
    remove_headers: &[String],
) -> io::Result<Vec<u8>> {
    let HttpMessage {
        buf,
        head,
        hdr_ranges,
        read_len,
        framing,
    } = message;
    let body_prefix = match framing {
        BodyFraming::ContentLength(n) => read_len.saturating_sub(head.header_end).min(n),
        BodyFraming::None | BodyFraming::Chunked | BodyFraming::UntilEof => 0,
    };
    let buf = write_head(
        buf,
        dst,
        head,
        hdr_ranges,
        add_headers,
        remove_headers,
        body_prefix,
    )
    .await?;

    match framing {
        BodyFraming::ContentLength(n) => {
            let already_read = read_len.saturating_sub(head.header_end);
            send_n(buf, src, dst, n.saturating_sub(already_read)).await
        }
        BodyFraming::Chunked => relay_chunked(buf, head.header_end, read_len, src, dst).await,
        BodyFraming::None => Ok(buf),
        BodyFraming::UntilEof => relay_until_eof(buf, head.header_end, read_len, src, dst).await,
    }
}

fn response_status(buf: &[u8], head: &http::HttpHead) -> Option<u16> {
    std::str::from_utf8(&buf[..head.start_of_header])
        .ok()?
        .split_ascii_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

pub async fn forward_http(
    client: &mut TcpStream,
    forward_param: ForwardHttp,
    mut buf: Vec<u8>,
    hdr_buf: &mut [MaybeUninit<(Range<usize>, Range<usize>)>],
    client_req_head: &http::HttpHead,
    client_req_read_len: usize,
    client_keepalive_requested: bool,
) -> io::Result<(Vec<u8>, bool)> {
    // check out upstream connection
    let mut backend = net::get_conn(&forward_param.upstream).await?;
    let request_is_head = buf[..client_req_head.start_of_header].starts_with(b"HEAD ");

    let (request_add_headers, request_remove_headers) = forward_param
        .headers_patch
        .as_ref()
        .map_or((&[][..], &[][..]), |patch| {
            (&patch.add_headers[..], &patch.remove_headers[..])
        });
    let request_framing = if let Some(n) = client_req_head.content_len {
        BodyFraming::ContentLength(n)
    } else if client_req_head.chunked {
        BodyFraming::Chunked
    } else {
        BodyFraming::None
    };
    buf = relay_http_message(
        client,
        &mut backend,
        HttpMessage {
            buf,
            head: client_req_head,
            hdr_ranges: hdr_buf,
            read_len: client_req_read_len,
            framing: request_framing,
        },
        request_add_headers,
        request_remove_headers,
    )
    .await?;

    // read upstream response head
    let (b, upstream_head, _upstream_req_line, upstream_read_len) =
        http::read_headers(buf, &mut backend, hdr_buf).await?;
    buf = b;
    let upstream_status = response_status(&buf, &upstream_head);
    let response_has_no_body = request_is_head
        || upstream_status
            .is_some_and(|status| (100..200).contains(&status) || matches!(status, 204 | 304));

    // a response with no Content-Length and no chunked framing has no
    // reliable end-of-body signal short of closing the connection, so
    // keep-alive needs one or the other
    let client_keepalive = forward_param.keep_alive
        && client_keepalive_requested
        && (upstream_head.content_len.is_some() || upstream_head.chunked);

    let response_add_headers = [(
        "Connection".to_string(),
        if client_keepalive {
            "Keep-Alive"
        } else {
            "Close"
        }
        .to_string(),
    )];
    let response_remove_headers = ["Connection".to_string()];
    let response_framing = if response_has_no_body {
        BodyFraming::None
    } else if let Some(n) = upstream_head.content_len {
        BodyFraming::ContentLength(n)
    } else if upstream_head.chunked {
        BodyFraming::Chunked
    } else {
        BodyFraming::UntilEof
    };
    buf = relay_http_message(
        &mut backend,
        client,
        HttpMessage {
            buf,
            head: &upstream_head,
            hdr_ranges: hdr_buf,
            read_len: upstream_read_len,
            framing: response_framing,
        },
        &response_add_headers,
        &response_remove_headers,
    )
    .await?;

    // Close-delimited responses consume the upstream connection through EOF.
    if !matches!(response_framing, BodyFraming::UntilEof) {
        net::put_conn(forward_param.upstream, backend).await;
    }

    Ok((buf, client_keepalive))
}

pub async fn handle_req(
    mut client: TcpStream,
    peer_addr: SocketAddr,
    state: Rc<LuaConf>,
) -> io::Result<()> {
    let client_t = Client::new(peer_addr.ip(), peer_addr.port());

    let onaccept_handler_resp = resolve::run_on_accept(&state, &client_t)?;
    if let Some(resp) = onaccept_handler_resp {
        match resp {
            // completely drop or we can send a custom response to the client
            OnAcceptHandlerResp::Deny(_x) => return Ok(()),
            OnAcceptHandlerResp::CustomHttpResponse(resp) => {
                send_custom_http_response(
                    &mut client,
                    resp.status,
                    &resp.reason,
                    &resp.headers,
                    resp.body.as_bytes(),
                )
                .await?;
                return Ok(());
            }
            _ => { /* continue */ }
        }
    }

    let mut buf = vec![0u8; 8 * 1024];
    let mut hdr_buf: [MaybeUninit<(Range<usize>, Range<usize>)>; MAX_HEADERS] =
        [const { MaybeUninit::uninit() }; MAX_HEADERS];

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

        // decide
        let on_request_handler_resp =
            resolve::run_on_request(host, client_t.clone(), req, &state).await?;
        match on_request_handler_resp {
            // completely drop or we can send a custom response to the client
            OnRequestHandlerResp::Deny(_x) => return Ok(()),
            OnRequestHandlerResp::CustomHttpResponse(resp) => {
                send_custom_http_response(
                    &mut client,
                    resp.status,
                    &resp.reason,
                    &resp.headers,
                    resp.body.as_bytes(),
                )
                .await?;
                return Ok(());
            }
            OnRequestHandlerResp::ForwardHttp(fwd_param) => {
                let (b, client_keepalive) = forward_http(
                    &mut client,
                    fwd_param,
                    buf,
                    &mut hdr_buf,
                    &head,
                    read_len,
                    keep_alive_requested,
                )
                .await?;
                buf = b;
                if !client_keepalive {
                    return Ok(());
                }
                continue;
            }
        }
    }
}
pub async fn send_custom_http_response<K: AsRef<str>>(
    client: &mut TcpStream,
    status: u16,
    reason: &str,
    headers: &[(K, K)],
    body: &[u8],
) -> io::Result<()> {
    if !(200..=599).contains(&status)
        || reason
            .bytes()
            .any(|byte| !(byte == b'\t' || (b' '..=b'~').contains(&byte) || byte >= 0x80))
        || (matches!(status, 204 | 304) && !body.is_empty())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid HTTP response status line",
        ));
    }

    let mut response = format!("HTTP/1.1 {status} {reason}\r\n").into_bytes();
    for (name, value) in headers {
        let valid_name = !name.as_ref().is_empty()
            && name
                .as_ref()
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte));
        if !valid_name
            || value
                .as_ref()
                .bytes()
                .any(|byte| !(byte == b'\t' || (b' '..=b'~').contains(&byte) || byte >= 0x80))
            || name.as_ref().eq_ignore_ascii_case("content-length")
            || name.as_ref().eq_ignore_ascii_case("connection")
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid or reserved HTTP response header",
            ));
        }
        response.extend_from_slice(name.as_ref().as_bytes());
        response.extend_from_slice(b": ");
        response.extend_from_slice(value.as_ref().as_bytes());
        response.extend_from_slice(b"\r\n");
    }
    if !matches!(status, 204 | 304) {
        response.extend_from_slice(format!("Content-Length: {}\r\n", body.len()).as_bytes());
    }
    response.extend_from_slice(b"Connection: close\r\n\r\n");
    response.extend_from_slice(body);

    let (result, _) = client.write_all(response).await;
    result.map(|_| ())
}
