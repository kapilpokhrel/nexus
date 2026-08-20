use crate::lua_config::Request;
use monoio::buf::IoBufMut;
use monoio::io::AsyncReadRent;
use monoio::net::TcpStream;
use std::io;
use std::mem::MaybeUninit;
use std::ops::Range;

fn range_of(buf: &[u8], sub: &[u8]) -> Range<usize> {
    let start = sub.as_ptr() as usize - buf.as_ptr() as usize;
    start..start + sub.len()
}

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
}

pub fn parse_host_requset<'a>(
    buf: &'a [u8],
    hdr: &[MaybeUninit<(Range<usize>, Range<usize>)>; 64],
    head: &HttpHead,
    req_line: &RequestInfo,
) -> (&'a str, Request) {
    let n = head.header_count.min(64);
    let mut headers = Vec::with_capacity(n);
    for h in hdr.iter().take(n) {
        headers.push(unsafe { h.assume_init_read() });
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
