use monoio::net::{TcpListener, TcpStream};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::io;
use std::os::fd::AsRawFd;
use std::time::Duration;

pub const MAX_IDLE: usize = 32;

thread_local! {
    static CONN_POOL: RefCell<HashMap<String, VecDeque<TcpStream>>> = RefCell::new(HashMap::new());
}

fn is_stream_alive(stream: &TcpStream) -> bool {
    let fd = stream.as_raw_fd();
    let mut buf = [0u8; 1];

    // MSG_PEEK: inspect buffer without pulling data out.
    // MSG_DONTWAIT: instantly return if no data exists instead of waiting.
    let res = unsafe {
        libc::recv(
            fd,
            buf.as_mut_ptr() as *mut libc::c_void,
            1,
            libc::MSG_PEEK | libc::MSG_DONTWAIT,
        )
    };

    if res > 0 {
        true
    } else if res == 0 {
        false
    } else {
        let err = io::Error::last_os_error();
        matches!(err.kind(), io::ErrorKind::WouldBlock)
    }
}

pub async fn get_conn(backend_addr: &String) -> io::Result<TcpStream> {
    let s = CONN_POOL.with(|p| {
        p.borrow_mut()
            .get_mut(backend_addr)
            .and_then(|v| v.pop_back())
    });
    let stream = if let Some(stream) = s
        && is_stream_alive(&stream)
    {
        stream
    } else {
        let s = TcpStream::connect(backend_addr).await?;
        let _ = s.set_nodelay(true);
        s
    };
    Ok(stream)
}

pub async fn put_conn(backend_addr: String, s: TcpStream) {
    CONN_POOL.with_borrow_mut(|h| {
        let vq = h.entry(backend_addr).or_insert(VecDeque::new());
        if vq.len() < MAX_IDLE {
            vq.push_back(s)
        }
    });
}

pub fn enable_defer_accept(listener: &TcpListener, timeout: Duration) -> io::Result<()> {
    let fd = listener.as_raw_fd();
    let seconds = timeout.as_secs() as libc::c_int;

    unsafe {
        let ret = libc::setsockopt(
            fd,
            libc::IPPROTO_TCP,
            libc::TCP_DEFER_ACCEPT,
            &seconds as *const libc::c_int as *const libc::c_void,
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        );

        if ret == -1 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}
