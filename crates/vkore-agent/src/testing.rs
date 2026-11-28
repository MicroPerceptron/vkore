//! A scripted chat server for tests: it answers each request with the next reply.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::Value;

use crate::config::Endpoint;

pub type Answer = (u16, &'static str, String);

#[derive(Clone)]
pub struct Seen {
    pub head: String,
    pub body: Value,
}

pub struct Server {
    base: String,
    seen: Arc<Mutex<Vec<Seen>>>,
}

/// A stream of events, ended as servers end it.
pub fn sse(events: &[&str]) -> Answer {
    let mut body = String::from(": keep-alive\n\n");
    for event in events {
        body.push_str(&format!("data: {event}\n\n"));
    }
    body.push_str("data: [DONE]\n\n");
    (200, "text/event-stream", body)
}

fn read_request(stream: &TcpStream) -> Option<Seen> {
    let mut reader = BufReader::new(stream);
    let mut head = String::new();
    let mut length = 0;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().ok()?;
        }
        if line == "\r\n" {
            break;
        }
        head.push_str(&line);
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).ok()?;
    Some(Seen {
        head,
        body: serde_json::from_slice(&body).ok()?,
    })
}

impl Server {
    fn start(mut serve: impl FnMut(TcpStream, usize) + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        thread::spawn(move || {
            for (at, stream) in listener.incoming().enumerate() {
                let Ok(stream) = stream else { return };
                serve(stream, at);
            }
        });
        Self {
            base,
            seen: Arc::default(),
        }
    }

    pub fn new(answers: Vec<Answer>) -> Self {
        let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
        let record = seen.clone();
        let mut server = Self::start(move |mut stream, at| {
            let Some(request) = read_request(&stream) else {
                return;
            };
            record.lock().unwrap().push(request);
            let (status, kind, body) =
                answers
                    .get(at)
                    .cloned()
                    .unwrap_or((500, "text/plain", "no more answers".into()));
            let _ = write!(
                stream,
                "HTTP/1.1 {status} Scripted\r\ncontent-type: {kind}\r\nlocation: /elsewhere\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
        });
        server.seen = seen;
        server
    }

    /// A server that takes a request and never answers.
    pub fn silent() -> Self {
        Self::start(|stream, _| {
            let _ = read_request(&stream);
            thread::sleep(Duration::from_secs(10));
        })
    }

    pub fn endpoint(&self, key: Option<&str>) -> Endpoint {
        Endpoint::new(&self.base, "scripted", key).unwrap()
    }

    pub fn request(&self, at: usize) -> Seen {
        self.seen.lock().unwrap()[at].clone()
    }

    pub fn requests(&self) -> usize {
        self.seen.lock().unwrap().len()
    }
}
