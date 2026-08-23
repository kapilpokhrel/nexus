use crate::http::{HttpHead, MAX_HEADERS};
use crate::resolve::ExtraHeaders;
use monoio::buf::{IoBuf, IoBufMut};
use monoio::io::{AsyncReadRent, AsyncWriteRentExt};
use monoio::net::TcpStream;
use std::io;
use std::mem::MaybeUninit;
use std::ops::Range;

// i think there is a problem here
// we will lose our buffer on error
pub async fn fwd(
    buf: Vec<u8>,
    dst: &mut TcpStream,
    n: usize,
    offset: usize,
) -> io::Result<Vec<u8>> {
    let (res, view) = dst.write_all(buf.slice(offset..(offset + n))).await;
    res?;
    Ok(view.into_inner())
}

pub async fn send_n(
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

/// `body_prefix` is how many body bytes already sit in `buf` right after the
/// head; they go out in the same write so the head and the start of the body
/// are one segment instead of two.
pub async fn write_request_head(
    buf: Vec<u8>,
    dst: &mut TcpStream,
    head: &HttpHead,
    extra_headers: ExtraHeaders,
    body_prefix: usize,
) -> io::Result<Vec<u8>> {
    let mut temp_buf: Vec<u8> = Vec::with_capacity(256 + head.header_offset);
    temp_buf.extend_from_slice(&buf[0..head.start_of_header]);
    if let Some(headers) = extra_headers {
        for (k, v) in headers {
            temp_buf.extend_from_slice(k.as_bytes());
            temp_buf.extend_from_slice(b": ");
            temp_buf.extend_from_slice(v.as_bytes());
            temp_buf.extend_from_slice(b"\r\n");
        }
    }
    temp_buf.extend_from_slice(&buf[head.start_of_header..head.header_offset]);
    temp_buf.extend_from_slice(&buf[head.header_offset..head.header_offset + body_prefix]);
    let (res, _) = dst.write_all(temp_buf).await;
    res?;
    Ok(buf)
}

/// Reads more bytes from `src` into `buf`, growing it first if it's already
/// full. Returns the new valid length. Errors on EOF, matching `read_headers`.
async fn read_more(
    mut buf: Vec<u8>,
    valid: usize,
    src: &mut TcpStream,
) -> io::Result<(Vec<u8>, usize)> {
    if valid == buf.len() {
        buf.resize(buf.len() * 2, 0);
    }
    let (res, slice) = src.read(buf.slice_mut(valid..)).await;
    buf = slice.into_inner();
    let n = res?;
    if n == 0 {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "eof"));
    }
    Ok((buf, valid + n))
}

/// Finds the next CRLF at or after `pos` within `buf[..valid]`, reading more
/// from `src` as needed. Returns the index of the CRLF's first byte.
async fn find_crlf(
    mut buf: Vec<u8>,
    pos: usize,
    mut valid: usize,
    src: &mut TcpStream,
) -> io::Result<(Vec<u8>, usize, usize)> {
    loop {
        if let Some(i) = buf[pos..valid].windows(2).position(|w| w == b"\r\n") {
            return Ok((buf, valid, pos + i));
        }
        let (b, v) = read_more(buf, valid, src).await?;
        buf = b;
        valid = v;
    }
}

/// Relays exactly `n` bytes starting at absolute position `start` from `src`
/// to `dst`, using whatever's already buffered in `buf[start..valid]` first
/// and streaming the rest straight through if `n` isn't fully buffered.
/// Returns `(buf, pos, valid)` for the byte immediately after the relayed
/// span; once anything had to be streamed fresh, nothing is left buffered
/// (pos and valid both come back 0), since streaming reads land at the front
/// of `buf` rather than preserving its earlier contents.
async fn relay_span(
    mut buf: Vec<u8>,
    start: usize,
    valid: usize,
    n: usize,
    src: &mut TcpStream,
    dst: &mut TcpStream,
) -> io::Result<(Vec<u8>, usize, usize)> {
    let have = valid.saturating_sub(start).min(n);
    buf = fwd(buf, dst, have, start).await?;
    if have < n {
        buf = send_n(buf, src, dst, n - have).await?;
        Ok((buf, 0, 0))
    } else {
        Ok((buf, start + have, valid))
    }
}

fn parse_chunk_size(line: &[u8]) -> io::Result<usize> {
    let bad_size = || io::Error::new(io::ErrorKind::InvalidData, "bad chunk size");
    let s = std::str::from_utf8(line).map_err(|_| bad_size())?;
    // a chunk-size line may carry `;`-separated extensions after the size,
    // which we forward verbatim but don't need to interpret
    let s = s.split(';').next().unwrap().trim();
    usize::from_str_radix(s, 16).map_err(|_| bad_size())
}

/// Relays the trailer section that follows the last-chunk: zero or more
/// CRLF-terminated header lines, ending in a blank line. Forwarded verbatim
/// without being parsed into name/value pairs, since we don't act on them.
async fn relay_trailers(
    mut buf: Vec<u8>,
    mut pos: usize,
    mut valid: usize,
    src: &mut TcpStream,
    dst: &mut TcpStream,
) -> io::Result<Vec<u8>> {
    loop {
        let (b, v, line_end) = find_crlf(buf, pos, valid, src).await?;
        buf = b;
        valid = v;
        let blank = line_end == pos;
        let (b, p, v) = relay_span(buf, pos, valid, line_end + 2 - pos, src, dst).await?;
        buf = b;
        pos = p;
        valid = v;
        if blank {
            return Ok(buf);
        }
    }
}

/// Relays an HTTP chunked-encoded body from `src` to `dst` verbatim (chunk-size
/// lines, chunk data, and trailers included byte for byte) — the payload
/// itself is never interpreted, only its framing, so we know where the body
/// ends. `buf[pos..valid]` holds whatever bytes of the body were already read
/// alongside the preceding headers.
pub async fn relay_chunked(
    mut buf: Vec<u8>,
    mut pos: usize,
    mut valid: usize,
    src: &mut TcpStream,
    dst: &mut TcpStream,
) -> io::Result<Vec<u8>> {
    loop {
        // chunk-size line, e.g. "1a\r\n", or "0\r\n" for the last chunk
        let (b, v, line_end) = find_crlf(buf, pos, valid, src).await?;
        buf = b;
        valid = v;
        let size = parse_chunk_size(&buf[pos..line_end])?;
        let data_start = line_end + 2;

        let (b, p, v) = relay_span(buf, pos, valid, data_start - pos, src, dst).await?;
        buf = b;
        pos = p;
        valid = v;

        if size == 0 {
            return relay_trailers(buf, pos, valid, src, dst).await;
        }

        // chunk-data followed by its trailing CRLF
        let (b, p, v) = relay_span(buf, pos, valid, size + 2, src, dst).await?;
        buf = b;
        pos = p;
        valid = v;
    }
}

/// Writes the response status line followed by the upstream's headers, with
/// `Connection` overridden to `keep_alive`'s value. `Connection` is hop-by-hop:
/// whatever the upstream sent describes the proxy<->upstream leg, not this one,
/// so any upstream-supplied `Connection` header is dropped rather than forwarded.
pub async fn write_response_head(
    buf: Vec<u8>,
    dst: &mut TcpStream,
    head: &HttpHead,
    hdr_ranges: &[MaybeUninit<(Range<usize>, Range<usize>)>],
    keep_alive: bool,
    body_prefix: usize,
) -> io::Result<Vec<u8>> {
    let mut temp_buf: Vec<u8> = Vec::with_capacity(256 + head.header_offset);
    temp_buf.extend_from_slice(&buf[0..head.start_of_header]);
    temp_buf.extend_from_slice(if keep_alive {
        b"Connection: Keep-Alive\r\n"
    } else {
        b"Connection: Close\r\n"
    });
    for h_range in hdr_ranges.iter().take(head.header_count.min(MAX_HEADERS)) {
        let (name_r, val_r) = unsafe { h_range.assume_init_ref() };
        if buf[name_r.clone()].eq_ignore_ascii_case(b"Connection") {
            continue;
        }
        temp_buf.extend_from_slice(&buf[name_r.clone()]);
        temp_buf.extend_from_slice(b": ");
        temp_buf.extend_from_slice(&buf[val_r.clone()]);
        temp_buf.extend_from_slice(b"\r\n");
    }
    temp_buf.extend_from_slice(b"\r\n");
    temp_buf.extend_from_slice(&buf[head.header_offset..head.header_offset + body_prefix]);
    let (res, _) = dst.write_all(temp_buf).await;
    res?;
    Ok(buf)
}
