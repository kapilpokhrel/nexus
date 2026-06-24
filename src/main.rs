use monoio::buf::{IoBuf, IoBufMut};
use monoio::io::{AsyncReadRent, AsyncWriteRentExt};
use monoio::net::{TcpListener, TcpStream};
use std::io;
use std::mem::MaybeUninit;
use std::ops::Range;
mod lua_config;

async fn host_backend<'a>(hdrs: &'_ [(&'a str, &'a str); 64]) -> io::Result<TcpStream> {
    let host = hdrs
        .iter()
        .find(|h| h.0.eq_ignore_ascii_case("Host"))
        .map(|h| h.1.trim())
        .ok_or(io::Error::new(io::ErrorKind::InvalidData, "host not found"))?;

    TcpStream::connect(host).await
}

#[allow(unused_variables)]
#[monoio::main(threads = 8)]
async fn main() {
    let listener = TcpListener::bind("127.0.0.1:5000").unwrap();
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        monoio::spawn(async move {
            let _ = forward_http(stream).await;
        });
    }
}

async fn forward_http(mut client: TcpStream) -> io::Result<()> {
    let buf = vec![0u8; 8 * 1024];
    let mut hdr_buf: [MaybeUninit<(Range<usize>, Range<usize>)>; 64] =
        [const { MaybeUninit::uninit() }; 64];

    // request
    let (buf, hdr_end, read_len, cl, chunked, h_count) =
        read_headers(buf, &mut client, &mut hdr_buf[..]).await?;
    let hdr = get_hdrs(&buf, &hdr_buf, h_count);
    let mut backend = host_backend(&hdr).await?;

    let in_buf = read_len.saturating_sub(hdr_end);
    let buf = if let Some(n) = cl {
        let buf = fwd(buf, &mut backend, hdr_end + in_buf.min(n)).await?;
        send_n(buf, &mut client, &mut backend, n.saturating_sub(in_buf)).await?
    } else if chunked {
        // will probably have to use httparse::parse_chunk
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "we don't handle chunked yet",
        ));
    } else {
        fwd(buf, &mut backend, hdr_end).await?
    };

    // response
    let (buf, hdr_end, read_len, cl, chunked, _h_count) =
        read_headers(buf, &mut backend, &mut hdr_buf).await?;

    let in_buf = read_len.saturating_sub(hdr_end);
    if let Some(n) = cl {
        let buf = fwd(buf, &mut client, hdr_end + in_buf.min(n)).await?;
        let _ = send_n(buf, &mut backend, &mut client, n.saturating_sub(in_buf)).await?;
    } else if chunked {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "we don't handle chunked yet",
        ));
    } else {
        let _ = fwd(buf, &mut client, hdr_end).await?;
    }
    Ok(())
}

fn range_of(buf: &[u8], sub: &[u8]) -> Range<usize> {
    let start = sub.as_ptr() as usize - buf.as_ptr() as usize;
    start..start + sub.len()
}

fn get_hdrs<'a>(
    buf: &'a [u8],
    hdr: &[MaybeUninit<(Range<usize>, Range<usize>)>; 64],
    n: usize,
) -> [(&'a str, &'a str); 64] {
    let mut out = [("", ""); 64];
    for i in 0..n.min(64) {
        let (nr, vr) = unsafe { hdr[i].assume_init_read() };
        out[i] = (unsafe { std::str::from_utf8_unchecked(&buf[nr]) }, unsafe {
            std::str::from_utf8_unchecked(&buf[vr])
        });
    }
    out
}

/// returns (buf, header_end, total_bytes_read, content_length?, chunked?, header_count)
async fn read_headers(
    mut buf: Vec<u8>,
    stream: &mut TcpStream,
    hdr_ranges: &mut [MaybeUninit<(Range<usize>, Range<usize>)>],
) -> io::Result<(Vec<u8>, usize, usize, Option<usize>, bool, usize)> {
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
        let se = se + 2;

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
                let n = parsed.len();
                return Ok((buf, header_end, ptr, cl, chunked, n));
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

async fn fwd(buf: Vec<u8>, dst: &mut TcpStream, n: usize) -> io::Result<Vec<u8>> {
    let (res, view) = dst.write_all(buf.slice(..n)).await;
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
