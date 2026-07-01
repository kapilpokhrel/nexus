use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

const TOTAL_REQUESTS: u64 = 100000;
const CONCURRENCY: u64 = 100;
const PROXY_ADDR: &str = "127.0.0.1:5000";
const BACKEND_ADDR: &str = "127.0.0.1:8000";
const BODY_SIZE: usize = 8192;

const REQUEST: &[u8] = concat!(
    "GET / HTTP/1.1\r\n",
    "Host: example.com\r\n",
    //"Connection: close\r\n",
    "Accept: */*\r\n",
    "Content-Length: 8192\r\n\r\n",
).as_bytes();

static REQUEST_BODY: [u8; BODY_SIZE] = [b'X'; BODY_SIZE];

fn start_server() {
    let listener = Arc::new(TcpListener::bind(BACKEND_ADDR).unwrap());
    static RESP: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 34\r\nConnection: keep-alive\r\n\r\nHello from backend server on port 8000\n";

    // 128 connection pools
    for _ in 0..128 {
        let listener = listener.clone();
        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                let mut stream = match listener.accept() {
                    Ok((s, _)) => s,
                    Err(_) => continue,
                };
                'conn: loop {
                    let mut ptr = 0;
                    let (header_end, body_size) = loop {
                        match stream.read(&mut buf[ptr..]) {
                            Ok(0) => break 'conn,
                            Ok(n) => ptr += n,
                            Err(_) => break 'conn,
                        }
                        let Some(sl) = buf[..ptr].windows(2).position(|w| w == b"\r\n") else { continue };
                        let se = sl + 2;
                        let Some(he) = buf[se..ptr].windows(4).position(|w| w == b"\r\n\r\n") else { continue };
                        let header_end = se + he + 4;
                        let mut raw = [httparse::EMPTY_HEADER; 64];
                        if let Ok(httparse::Status::Complete((_, parsed))) =
                            httparse::parse_headers(&buf[se..header_end], &mut raw)
                        {
                            let body_size = parsed
                                .iter()
                                .find(|h| h.name.eq_ignore_ascii_case("Content-Length"))
                                .and_then(|h| std::str::from_utf8(h.value).ok()?.trim().parse::<usize>().ok())
                                .unwrap_or(0);
                            break (header_end, body_size);
                        }
                    };

                    let in_buf = ptr.saturating_sub(header_end);
                    let mut rem = body_size.saturating_sub(in_buf);
                    while rem > 0 {
                        let sz = rem.min(buf.len());
                        match stream.read(&mut buf[..sz]) {
                            Ok(0) => break 'conn,
                            Ok(n) => rem -= n,
                            Err(_) => break 'conn,
                        }
                    }
                    if stream.write_all(RESP).is_err() {
                        break 'conn;
                    }
                }
            }
        });
    }
}

struct ProxyGuard(Option<Child>);

impl ProxyGuard {
    fn start() -> Self {
        let child = Command::new("cargo")
            .args(["run", "--release", "--bin", "conduit", "-q"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("failed to start proxy via cargo");
        println!("  proxy started (pid {})", child.id());
        ProxyGuard(Some(child))
    }
}

impl Drop for ProxyGuard {
    fn drop(&mut self) {
        if let Some(ref mut child) = self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn try_connect(addr: &str) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(30) {
        if TcpStream::connect(addr).is_ok() {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    panic!("timeout waiting for {addr}");
}

fn worker(count: u64, results: &mut Vec<f64>) -> u64 {
    let mut errors = 0u64;
    let mut buf = [0u8; 4096];

    for _ in 0..count {
        let start = Instant::now();
        match TcpStream::connect(PROXY_ADDR) {
            Ok(mut stream) => {
                if stream.write_all(REQUEST).is_err()
                    || stream.write_all(&REQUEST_BODY).is_err()
                {
                    errors += 1;
                    continue;
                }
                match stream.read(&mut buf) {
                    Ok(n) if n > 0 => {
                        results.push(start.elapsed().as_secs_f64() * 1000.0);
                    }
                    _ => errors += 1,
                }
            }
            Err(_) => errors += 1,
        }
    }
    errors
}

fn main() {
    start_server();

    println!("spawning proxy...");
    let _proxy = ProxyGuard::start();
    try_connect(PROXY_ADDR);

    let per_worker = TOTAL_REQUESTS / CONCURRENCY;
    let test_start = Instant::now();
    let all_errors = Arc::new(AtomicU64::new(0));
    let all_results: Arc<std::sync::Mutex<Vec<f64>>> =
        Arc::new(std::sync::Mutex::new(Vec::with_capacity(TOTAL_REQUESTS as usize)));

    let mut handles = Vec::with_capacity(CONCURRENCY as usize);
    for _ in 0..CONCURRENCY {
        let results = all_results.clone();
        let errors = all_errors.clone();
        handles.push(thread::spawn(move || {
            let mut local = Vec::with_capacity(per_worker as usize);
            let e = worker(per_worker, &mut local);
            results.lock().unwrap().extend(local);
            errors.fetch_add(e, Ordering::Relaxed);
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    let duration = test_start.elapsed();
    let mut results = all_results.lock().unwrap();
    results.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let n = results.len();
    if n == 0 {
        println!("\nno successful requests");
        return;
    }
    let avg = results.iter().sum::<f64>() / n as f64;
    let p95 = results[(n as f64 * 0.95) as usize];
    let p99 = results[(n as f64 * 0.99) as usize];

    println!("\n===== Benchmark Result =====");
    println!("Total Requests : {n}");
    println!("Concurrency    : {CONCURRENCY}");
    println!("Duration       : {:.2} sec", duration.as_secs_f64());
    println!("Requests/sec   : {:.2}", n as f64 / duration.as_secs_f64());
    println!("Average Latency: {avg:.2} ms");
    println!("P95 Latency    : {p95:.2} ms");
    println!("P99 Latency    : {p99:.2} ms");
    println!("Errors         : {}", all_errors.load(Ordering::Relaxed));
}
