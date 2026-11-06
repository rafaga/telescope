//! A small HTTP server on the loopback address for the tests: it answers
//! what it was told to and remembers what it was asked.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;

/// What the server answers to a request.
#[derive(Clone)]
pub(crate) struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Reply {
    pub(crate) fn json(status: u16, body: &str) -> Self {
        Self {
            status,
            headers: vec![(
                String::from("Content-Type"),
                String::from("application/json"),
            )],
            body: body.to_string(),
        }
    }

    pub(crate) fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    pub(crate) fn with_body(mut self, body: &str) -> Self {
        self.body = body.to_string();
        self
    }
}

/// A request the server received.
#[derive(Clone, Debug)]
pub(crate) struct Recorded {
    pub(crate) method: String,
    /// The path and the query.
    pub(crate) target: String,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: String,
}

impl Recorded {
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

struct State {
    /// The replies of each path, in the order they are given; the last one
    /// is repeated.
    routes: HashMap<String, Vec<Reply>>,
    served: HashMap<String, usize>,
    requests: Vec<Recorded>,
}

pub(crate) struct Mock {
    port: u16,
    state: Arc<Mutex<State>>,
}

impl Mock {
    /// A server that answers `routes`: a path (without the query) and the
    /// reply to give. A path given more than once is answered in order.
    pub(crate) fn start(routes: Vec<(&str, Reply)>) -> Self {
        let mut by_path: HashMap<String, Vec<Reply>> = HashMap::new();
        for (path, reply) in routes {
            by_path.entry(path.to_string()).or_default().push(reply);
        }
        let state = Arc::new(Mutex::new(State {
            routes: by_path,
            served: HashMap::new(),
            requests: Vec::new(),
        }));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let shared = Arc::clone(&state);
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let shared = Arc::clone(&shared);
                thread::spawn(move || serve(stream, &shared));
            }
        });
        Self { port, state }
    }

    pub(crate) fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// The address of a server that takes the connection and never answers.
    /// The thread that holds it ends with the test process.
    pub(crate) fn hanging_url() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            let mut held = Vec::new();
            for stream in listener.incoming().flatten() {
                held.push(stream);
            }
        });
        format!("http://127.0.0.1:{port}")
    }

    /// The address of a port nobody listens on: the connection is refused.
    pub(crate) fn refused_url() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        format!("http://127.0.0.1:{port}")
    }

    pub(crate) fn requests(&self) -> Vec<Recorded> {
        self.state.lock().unwrap().requests.clone()
    }
}

fn serve(mut stream: TcpStream, state: &Mutex<State>) {
    let mut received = Vec::new();
    let mut buffer = [0_u8; 4096];
    let (head_end, content_length) = loop {
        let Ok(read) = stream.read(&mut buffer) else {
            return;
        };
        if read == 0 {
            return;
        }
        received.extend_from_slice(&buffer[..read]);
        if let Some(position) = received.windows(4).position(|window| window == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&received[..position]).to_lowercase();
            let length = head
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .and_then(|value| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            break (position + 4, length);
        }
    };
    while received.len() < head_end + content_length {
        let Ok(read) = stream.read(&mut buffer) else {
            return;
        };
        if read == 0 {
            break;
        }
        received.extend_from_slice(&buffer[..read]);
    }
    let head = String::from_utf8_lossy(&received[..head_end]).to_string();
    let body = String::from_utf8_lossy(&received[head_end..]).to_string();
    let mut lines = head.lines();
    let mut first = lines.next().unwrap_or_default().split_whitespace();
    let method = first.next().unwrap_or_default().to_string();
    let target = first.next().unwrap_or_default().to_string();
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_string(), value.trim().to_string()))
        .collect();
    let path = target.split('?').next().unwrap_or_default().to_string();

    let reply = {
        let mut state = state.lock().unwrap();
        state.requests.push(Recorded {
            method,
            target,
            headers,
            body,
        });
        let served = state.served.entry(path.clone()).or_insert(0);
        let index = *served;
        *served += 1;
        state
            .routes
            .get(&path)
            .map(|replies| replies[index.min(replies.len() - 1)].clone())
            .unwrap_or_else(|| Reply::json(404, ""))
    };
    let mut response = format!("HTTP/1.1 {} Status\r\n", reply.status);
    for (name, value) in &reply.headers {
        response.push_str(&format!("{name}: {value}\r\n"));
    }
    response.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n{}",
        reply.body.len(),
        reply.body
    ));
    let _ = stream.write_all(response.as_bytes());
}
