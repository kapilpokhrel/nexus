use crate::lua_config::Request;
use monoio::buf::IoBufMut;
use monoio::io::AsyncReadRent;
use monoio::net::TcpStream;
use std::io;
use std::mem::MaybeUninit;
use std::ops::Range;

pub const MAX_HEADERS: usize = 64;

#[derive(Debug)]
pub struct HttpHead {
    pub content_len: Option<usize>,
    pub chunked: bool,
    pub start_of_header: usize,
    pub header_count: usize,
    pub header_offset: usize,
}

#[derive(Debug)]
pub struct RequestInfo {
    pub host: Option<Range<usize>>,
    pub method: Option<Range<usize>>,
    pub path: Option<Range<usize>>,
    pub version: Option<Range<usize>>,
}

pub fn range_of(buf: &[u8], sub: &[u8]) -> Range<usize> {
    let start = sub.as_ptr() as usize - buf.as_ptr() as usize;
    start..start + sub.len()
}

pub fn host_and_request<'a>(
    buf: &'a [u8],
    hdr: &[MaybeUninit<(Range<usize>, Range<usize>)>; MAX_HEADERS],
    head: &HttpHead,
    req_line: &RequestInfo,
) -> (&'a str, Request) {
    let n = head.header_count.min(MAX_HEADERS);
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

/// Whether the client's request indicates the connection should be kept open
/// after this response: an explicit `Connection` header wins, otherwise it
/// falls back to the protocol default (persistent for HTTP/1.1+, not for HTTP/1.0).
pub fn client_wants_keep_alive(buf: &[u8], req_line: &RequestInfo, req: &Request) -> bool {
    if let Some(v) = req.0.header("Connection") {
        let mut tokens = v.split(',').map(|t| t.trim());
        if tokens.clone().any(|t| t.eq_ignore_ascii_case("close")) {
            return false;
        }
        if tokens.any(|t| t.eq_ignore_ascii_case("keep-alive")) {
            return true;
        }
    }
    match &req_line.version {
        Some(r) => !buf[r.clone()].eq_ignore_ascii_case(b"HTTP/1.0"),
        None => false,
    }
}

pub async fn read_headers(
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

        let mut raw = [httparse::EMPTY_HEADER; MAX_HEADERS];
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

                let (method, path, version) = {
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
                            (Some(0..e), Some(e + 1..s), Some(s + 1..req.len()))
                        }
                        _ => (None, None, None),
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
                    RequestInfo { host, method, path, version },
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
