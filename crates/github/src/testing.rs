//! A tiny local HTTP server for tests: canned replies by method and path
//! prefix, and a record of what was sent. Never talks to the internet.

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    /// Header names lowercased.
    pub headers: Vec<(String, String)>,
    pub body: String,
}

pub struct MockServer {
    pub base: String,
    recorded: Arc<Mutex<Vec<Recorded>>>,
}

type Route = (&'static str, &'static str, u16, &'static str);

impl MockServer {
    /// Serves `routes` (`method`, path prefix, status, JSON body); the first
    /// matching route answers, anything else gets 404.
    pub fn start(routes: Vec<Route>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let log = recorded.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() || line.is_empty() {
                    continue;
                }
                let mut parts = line.split_whitespace();
                let method = parts.next().unwrap_or_default().to_string();
                let path = parts.next().unwrap_or_default().to_string();
                let mut headers = Vec::new();
                let mut length = 0usize;
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).is_err() || header.trim().is_empty() {
                        break;
                    }
                    if let Some((key, value)) = header.trim().split_once(':') {
                        let (key, value) =
                            (key.trim().to_ascii_lowercase(), value.trim().to_string());
                        if key == "content-length" {
                            length = value.parse().unwrap_or(0);
                        }
                        headers.push((key, value));
                    }
                }
                let mut body = vec![0; length];
                let _ = reader.read_exact(&mut body);
                let body = String::from_utf8_lossy(&body).into_owned();
                let (status, reply) = routes
                    .iter()
                    .find(|(m, prefix, _, _)| *m == method && path.starts_with(prefix))
                    .map(|(_, _, status, reply)| (*status, *reply))
                    .unwrap_or((404, r#"{"message":"Not Found"}"#));
                log.lock().unwrap().push(Recorded {
                    method,
                    path,
                    headers,
                    body,
                });
                let mut stream = stream;
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                    reply.len()
                );
            }
        });
        Self { base, recorded }
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.recorded.lock().unwrap().clone()
    }
}
