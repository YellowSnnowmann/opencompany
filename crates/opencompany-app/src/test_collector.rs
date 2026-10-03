//! A loopback HTTP collector for the shell's tests, hand-rolled on tokio so the
//! crate needs no new dev-dependency.
//!
//! It speaks just enough HTTP/1.1 to stand in for OpenPanel's `POST /track`:
//! read the request line, the headers and a `content-length` body, record all
//! of it, answer with a fixed status and close. Test-only (`#[cfg(test)]` at its
//! declaration in `lib.rs`).

use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

/// One request the collector received.
#[derive(Debug, Clone)]
pub struct Captured {
    /// The request target, e.g. `/track`.
    pub path: String,
    /// Header names (lowercased) and values, in arrival order.
    pub headers: Vec<(String, String)>,
    /// The body, as text.
    pub body: String,
}

impl Captured {
    /// The first header named `name` (case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.as_str())
    }

    /// The body parsed as JSON.
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).expect("the collector body is JSON")
    }
}

/// A running loopback collector.
pub struct TestCollector {
    /// The `http://127.0.0.1:<port>/track` URL to report to.
    pub url: String,
    requests: Arc<Mutex<Vec<Captured>>>,
    task: tokio::task::JoinHandle<()>,
}

impl TestCollector {
    /// Starts a collector that answers every request with `status`.
    pub async fn start(status: u16) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a loopback port");
        let url = format!("http://{}/track", listener.local_addr().unwrap());
        let requests: Arc<Mutex<Vec<Captured>>> = Arc::default();
        let sink = requests.clone();
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let sink = sink.clone();
                tokio::spawn(async move {
                    let _ = serve_one(stream, status, sink).await;
                });
            }
        });
        Self {
            url,
            requests,
            task,
        }
    }

    /// Everything received so far.
    pub fn requests(&self) -> Vec<Captured> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for TestCollector {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve_one(
    mut stream: tokio::net::TcpStream,
    status: u16,
    sink: Arc<Mutex<Vec<Captured>>>,
) -> std::io::Result<()> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(at) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break at;
        }
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Ok(());
        }
        buffer.extend_from_slice(&chunk[..read]);
    };
    let head = String::from_utf8_lossy(&buffer[..head_end]).into_owned();
    let mut lines = head.lines();
    let path = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("")
        .to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect();
    let length = headers
        .iter()
        .find(|(key, _)| key == "content-length")
        .and_then(|(_, value)| value.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = buffer[head_end + 4..].to_vec();
    while body.len() < length {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..read]);
    }
    sink.lock().unwrap().push(Captured {
        path,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    });
    let reply = format!("HTTP/1.1 {status} X\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
    stream.write_all(reply.as_bytes()).await?;
    stream.shutdown().await
}
