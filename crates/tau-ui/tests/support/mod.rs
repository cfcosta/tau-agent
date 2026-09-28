//! A fake GitHub for tests: answers each request by its request line,
//! and keeps every request whole.

use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{Arc, Mutex},
};

/// A fake GitHub on localhost: `answer` maps a request line
/// (`POST /login/device/code`) to a status and a JSON body.
pub fn fake(
    answer: impl Fn(&str) -> (u16, serde_json::Value) + Send + 'static,
) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut request = Vec::new();
            let mut buffer = [0; 4096];
            // Headers, then as much body as they announce.
            loop {
                let n = stream.read(&mut buffer).unwrap_or(0);
                request.extend_from_slice(&buffer[..n]);
                let text = String::from_utf8_lossy(&request);
                if let Some(end) = text.find("\r\n\r\n") {
                    let length = text
                        .lines()
                        .find_map(|line| {
                            line.to_lowercase()
                                .strip_prefix("content-length: ")
                                .and_then(|n| n.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length || n == 0 {
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            let text = String::from_utf8_lossy(&request).into_owned();
            let line = text.lines().next().unwrap_or_default();
            let line = line.rsplit_once(' ').map_or(line, |(l, _)| l);
            log.lock().unwrap().push(text.clone());
            let (status, body) = answer(line);
            let body = body.to_string();
            let _ = write!(
                stream,
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    (base, seen)
}
