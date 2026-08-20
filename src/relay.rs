use crate::http::HttpHead;
use monoio::buf::{IoBuf, IoBufMut};
use monoio::io::{AsyncReadRent, AsyncWriteRentExt};
use monoio::net::TcpStream;
use std::io;

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

pub async fn write_response_head(
    buf: Vec<u8>,
    dst: &mut TcpStream,
    head: &HttpHead,
    read_len: usize,
) -> io::Result<Vec<u8>> {
    let mut temp_buf: Vec<u8> = Vec::with_capacity(256 + read_len);
    temp_buf.extend_from_slice(&buf[0..head.start_of_header]);
    temp_buf.extend_from_slice(b"Connection: Close\r\n");
    temp_buf.extend_from_slice(&buf[head.start_of_header..]);
    let (res, _) = dst.write_all(temp_buf).await;
    res?;
    Ok(buf)
}
