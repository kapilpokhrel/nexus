use crate::http::{HttpHead, MAX_HEADERS};
use monoio::buf::{IoBuf, IoBufMut};
use monoio::io::{AsyncReadRent, AsyncWriteRentExt};
use monoio::net::TcpStream;
use std::io;
use std::mem::MaybeUninit;
use std::ops::Range;

// i think there is a problem here
// we will lose our buffer on error
pub async fn fwd(buf: Vec<u8>, dst: &mut TcpStream, n: usize, offset: usize) -> io::Result<Vec<u8>> {
    let (res, view) = dst.write_all(buf.slice(offset..(offset+n))).await;
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

pub async fn write_request_head(
    buf: Vec<u8>,
    dst: &mut TcpStream,
    head: &HttpHead,
    extra_headers: Option<Vec<(String, String)>>,
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
    let (res, _) = dst.write_all(temp_buf).await;
    res?;
    Ok(buf)
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
) -> io::Result<Vec<u8>> {
    let mut temp_buf: Vec<u8> = Vec::with_capacity(256 + head.header_offset);
    temp_buf.extend_from_slice(&buf[0..head.start_of_header]);
    temp_buf.extend_from_slice(if keep_alive {
        b"Connection: Keep-Alive\r\n"
    } else {
        b"Connection: Close\r\n"
    });
    for i in 0..head.header_count.min(MAX_HEADERS) {
        let (name_r, val_r) = unsafe { hdr_ranges[i].assume_init_ref() };
        if buf[name_r.clone()].eq_ignore_ascii_case(b"Connection") {
            continue;
        }
        temp_buf.extend_from_slice(&buf[name_r.clone()]);
        temp_buf.extend_from_slice(b": ");
        temp_buf.extend_from_slice(&buf[val_r.clone()]);
        temp_buf.extend_from_slice(b"\r\n");
    }
    temp_buf.extend_from_slice(b"\r\n");
    let (res, _) = dst.write_all(temp_buf).await;
    res?;
    Ok(buf)
}
