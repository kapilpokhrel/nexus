use std::io;
use monoio::buf::{IoBufMut, IoBuf};
use monoio::io::{AsyncReadRent, AsyncWriteRentExt};
use monoio::net::{TcpListener, TcpStream};

struct ForwardServer {
    stream: TcpStream
}

impl ForwardServer {
    async fn new(addr: impl Into<String>) -> io::Result<Self> {
        Ok(Self {
            stream: TcpStream::connect(addr.into()).await?
        })
    }
}

#[monoio::main(threads=8)]
async fn main() {
    let listener = TcpListener::bind("127.0.0.1:5000").unwrap();
    println!("listening");

    loop {
        let incoming = listener.accept().await;
        match incoming {
            Ok((stream, addr)) => {
                monoio::spawn(async move {
                    let forward_server = ForwardServer::new("127.0.0.1:8000").await.unwrap();
                    println!("accepted a connection from {}", addr);
                    let _ = forward(stream, forward_server).await;
                });
            }
            Err(e) => {
                println!("accepted connection failed: {}", e);
                return;
            }
        }
    }
}

async fn forward(mut stream: TcpStream, mut forward_s: ForwardServer) -> std::io::Result<()> {
    let mut buf: Vec<u8> = vec![0u8; 8 * 1024];
    let mut resp_buf: Vec<u8> = vec![0u8; 8 * 1024];

    let mut res;
    let mut len;
    loop {
        (len, buf) = parse_req_resp(buf, &mut stream, true).await;
        if len == 0 {
            return Ok(());
        }

        let write_slice;
        (res, write_slice) = forward_s.stream.write_all(buf.slice(0..len)).await;
        res?;
        buf = write_slice.into_inner();

        (len, resp_buf) = parse_req_resp(resp_buf, &mut forward_s.stream, false).await;
        if len == 0 {
            return Ok(());
        }

        let resp_write_slice;
        (res, resp_write_slice) = stream.write_all(resp_buf.slice(0..len)).await;
        res?;
        resp_buf = resp_write_slice.into_inner();
    }
}

pub fn get_content_length(headers: &[httparse::Header]) -> Option<usize> {
    headers
        .iter()
        .find(|p| p.name == "Content-Length")
        // ugliest shit
        .map(|p| str::from_utf8(p.value).unwrap().parse().unwrap())
}

async fn parse_req_resp(mut buf: Vec<u8>, stream: &mut TcpStream, is_req: bool) -> (usize, Vec<u8>) {
    let mut ptr = 0;
    let mut body_target = None;
    let mut len;

    loop {
        let (res, slice) = stream.read(buf.slice_mut(ptr..)).await;
        buf = slice.into_inner();

        len = res.unwrap();
        if len == 0 {
            return (ptr, buf);
        }
        ptr += len;

        if body_target.is_none() {
            let mut headers = [httparse::EMPTY_HEADER; 16];
            if is_req {
                let mut req = httparse::Request::new(&mut headers);

                if let Ok(p) = req.parse(&buf[..ptr]) {
                    match p {
                        httparse::Status::Partial => continue,
                        httparse::Status::Complete(header_len) => {
                            let content_len = get_content_length(&headers).unwrap_or(0);
                            body_target = Some(header_len + content_len);
                        }
                    }
                }
            } else {
                let mut res = httparse::Response::new(&mut headers);

                if let Ok(p) = res.parse(&buf[..ptr]) {
                    match p {
                        httparse::Status::Partial => continue,
                        httparse::Status::Complete(header_len) => {
                            let content_len = get_content_length(&headers).unwrap_or(0);
                            body_target = Some(header_len + content_len);
                        }
                    }
                }
            }
        }

        if let Some(target) = body_target
            && ptr >= target
        {
            return (target, buf);
        }
    }
}
