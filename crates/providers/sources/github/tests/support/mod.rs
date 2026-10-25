//! A loopback HTTP server that answers with canned GitHub-shaped responses.
#![allow(dead_code)]

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;

pub struct Canned {
    pub status: u16,
    pub headers: Vec<(&'static str, String)>,
    pub body: String,
}

pub fn canned(status: u16, body: &str) -> Canned {
    Canned {
        status,
        headers: Vec::new(),
        body: body.to_owned(),
    }
}

#[derive(Debug, Clone)]
pub struct Recorded {
    pub method: String,
    pub target: String,
    pub authorization: Option<String>,
    pub body: String,
}

pub struct Fake {
    pub base: String,
    pub requests: Arc<Mutex<Vec<Recorded>>>,
}

/// Serves `responses` in order, one per request, and records every request.
pub fn serve(responses: impl FnOnce(&str) -> Vec<Canned>) -> Fake {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let queue = Arc::new(Mutex::new(VecDeque::from(responses(&base))));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let (served, log) = (queue, requests.clone());
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let (queue, log) = (served.clone(), log.clone());
            thread::spawn(move || {
                let Some(request) = read_request(&mut stream) else {
                    return;
                };
                log.lock().unwrap().push(request);
                let response = queue.lock().unwrap().pop_front();
                let response = response.unwrap_or_else(|| canned(500, "{}"));
                let mut head = format!(
                    "HTTP/1.1 {} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n",
                    response.status,
                    response.body.len()
                );
                for (name, value) in &response.headers {
                    head.push_str(&format!("{name}: {value}\r\n"));
                }
                head.push_str("\r\n");
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(response.body.as_bytes());
            });
        }
    });
    Fake { base, requests }
}

fn read_request(stream: &mut std::net::TcpStream) -> Option<Recorded> {
    let mut data = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        let count = stream.read(&mut chunk).ok()?;
        if count == 0 {
            return None;
        }
        data.extend_from_slice(&chunk[..count]);
        if let Some(position) = data.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
    };
    let head = String::from_utf8_lossy(&data[..head_end]).into_owned();
    let mut lines = head.lines();
    let mut request_line = lines.next()?.split(' ');
    let method = request_line.next()?.to_owned();
    let target = request_line.next()?.to_owned();
    let header = |wanted: &str| {
        head.lines().skip(1).find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case(wanted)
                .then(|| value.trim().to_owned())
        })
    };
    let length: usize = header("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    while data.len() < head_end + length {
        let count = stream.read(&mut chunk).ok()?;
        if count == 0 {
            break;
        }
        data.extend_from_slice(&chunk[..count]);
    }
    Some(Recorded {
        method,
        target,
        authorization: header("authorization"),
        body: String::from_utf8_lossy(&data[head_end..]).into_owned(),
    })
}
