//! A small local control interface: other programs (a Stream Deck, a script, a hotkey tool) can
//! read and change Wavemix over plain HTTP on `127.0.0.1`. It is off by default, only listens on
//! this PC, and every request must carry the secret token shown in Settings. Requests are handed
//! to the app's UI thread, so they do exactly what the buttons do.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

/// Largest request we will read.
const MAX_REQUEST: usize = 8 * 1024;
/// How long the UI has to answer before the caller gets a timeout.
const ANSWER_WITHIN: Duration = Duration::from_millis(1500);

#[derive(Debug, PartialEq)]
pub struct Request {
    pub method: String,
    /// Path segments after the host, without the query: `["api", "strip", "0"]`.
    pub path: Vec<String>,
    pub query: HashMap<String, String>,
    /// The token the caller presented, from the Authorization header or the query.
    pub token: Option<String>,
}

/// A request waiting for the UI to answer it.
pub struct Job {
    pub request: Request,
    pub reply: Sender<(u16, String)>,
}

pub struct Remote {
    pub jobs: Receiver<Job>,
    stop: Arc<AtomicBool>,
    pub port: u16,
    pub token: String,
}

impl Drop for Remote {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

/// `%41` and `+` in a query string.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => match u8::from_str_radix(&s[i + 1..i + 3], 16) {
                Ok(b) => {
                    out.push(b);
                    i += 2;
                }
                Err(_) => out.push(b'%'),
            },
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Read the request line and headers of an HTTP/1.x request.
pub fn parse_request(text: &str) -> Option<Request> {
    let mut lines = text.split("\r\n");
    let first = lines.next()?;
    let mut parts = first.split_whitespace();
    let method = parts.next()?.to_uppercase();
    let target = parts.next()?;
    let (path, query_text) = target.split_once('?').unwrap_or((target, ""));
    let query: HashMap<String, String> = query_text
        .split('&')
        .filter(|kv| !kv.is_empty())
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (percent_decode(k).to_lowercase(), percent_decode(v))
        })
        .collect();
    let mut token = query.get("token").cloned();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("authorization") {
                let v = value.trim();
                token = v
                    .strip_prefix("Bearer ")
                    .or_else(|| v.strip_prefix("bearer "))
                    .map(str::to_string)
                    .or(token);
            }
        }
    }
    Some(Request {
        method,
        path: path
            .split('/')
            .filter(|s| !s.is_empty())
            .map(percent_decode)
            .collect(),
        query,
        token,
    })
}

/// Compare two secrets without stopping at the first difference.
pub fn token_matches(given: &str, expected: &str) -> bool {
    let (a, b) = (given.as_bytes(), expected.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// A fresh 128-bit secret, as 32 hex digits. The standard library seeds its hasher from the
/// operating system's random source, which is all the randomness this needs.
pub fn new_token() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    (0..2)
        .map(|n| {
            let mut h = RandomState::new().build_hasher();
            h.write_u64(n);
            h.write_u128(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_nanos()),
            );
            format!("{:016x}", h.finish())
        })
        .collect()
}

fn respond(stream: &mut TcpStream, code: u16, body: &str) {
    let status = match code {
        200 => "200 OK",
        400 => "400 Bad Request",
        401 => "401 Unauthorized",
        404 => "404 Not Found",
        504 => "504 Gateway Timeout",
        _ => "500 Internal Server Error",
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

fn serve(mut stream: TcpStream, token: &str, jobs: &Sender<Job>, wake: &(dyn Fn() + Send + Sync)) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") && buf.len() < MAX_REQUEST {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    let Some(request) = parse_request(&String::from_utf8_lossy(&buf)) else {
        return respond(
            &mut stream,
            400,
            r#"{"ok":false,"error":"unreadable request"}"#,
        );
    };
    if !request
        .token
        .as_deref()
        .is_some_and(|t| token_matches(t, token))
    {
        return respond(
            &mut stream,
            401,
            r#"{"ok":false,"error":"missing or wrong token"}"#,
        );
    }
    let (reply, answer) = mpsc::channel();
    if jobs.send(Job { request, reply }).is_err() {
        return respond(
            &mut stream,
            504,
            r#"{"ok":false,"error":"Wavemix is closing"}"#,
        );
    }
    wake();
    match answer.recv_timeout(ANSWER_WITHIN) {
        Ok((code, body)) => respond(&mut stream, code, &body),
        Err(_) => respond(
            &mut stream,
            504,
            r#"{"ok":false,"error":"Wavemix didn't answer"}"#,
        ),
    }
}

/// Start listening. `wake` is called when a request arrives, to get the UI to look at it.
pub fn start(
    port: u16,
    token: String,
    wake: impl Fn() + Send + Sync + 'static,
) -> Result<Remote, String> {
    let listener =
        TcpListener::bind(("127.0.0.1", port)).map_err(|e| format!("port {port}: {e}"))?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let (tx, rx) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let (stop_flag, secret) = (stop.clone(), token.clone());
    let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
    std::thread::Builder::new()
        .name("remote".into())
        .spawn(move || {
            while !stop_flag.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let (secret, tx, wake) = (secret.clone(), tx.clone(), wake.clone());
                        // One short-lived thread per request, so a slow caller can't block others.
                        std::thread::spawn(move || serve(stream, &secret, &tx, &*wake));
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(40)),
                }
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(Remote {
        jobs: rx,
        stop,
        port,
        token,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_read_with_query_and_token() {
        let r = parse_request(
            "GET /api/strip/2?gain_db=-6.5&mute=toggle&token=abc HTTP/1.1\r\nHost: x\r\n\r\n",
        )
        .unwrap();
        assert_eq!(r.method, "GET");
        assert_eq!(r.path, ["api", "strip", "2"]);
        assert_eq!(r.query["gain_db"], "-6.5");
        assert_eq!(r.query["mute"], "toggle");
        assert_eq!(r.token.as_deref(), Some("abc"));
    }

    #[test]
    fn a_bearer_header_works_too() {
        let r = parse_request("POST /api/engine HTTP/1.1\r\nAuthorization: Bearer s3cret\r\n\r\n")
            .unwrap();
        assert_eq!(r.token.as_deref(), Some("s3cret"));
        assert_eq!(r.method, "POST");
        assert!(parse_request("").is_none());
    }

    #[test]
    fn percent_escapes_and_plus_are_decoded() {
        let r = parse_request("GET /api/preset?name=Late%20night+mix%21 HTTP/1.1\r\n\r\n").unwrap();
        assert_eq!(r.query["name"], "Late night mix!");
    }

    #[test]
    fn tokens_compare_exactly() {
        assert!(token_matches("abcd", "abcd"));
        assert!(!token_matches("abcd", "abce"));
        assert!(!token_matches("abc", "abcd"));
        let (a, b) = (new_token(), new_token());
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
    }

    #[test]
    fn the_server_answers_with_a_good_token_and_refuses_without() {
        use std::io::{Read, Write};
        // Pick a free port by asking the OS.
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let remote = start(port, "tok".into(), || {}).unwrap();
        let ask = |path: &str| {
            let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
            write!(s, "GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
            let mut out = String::new();
            let _ = s.read_to_string(&mut out);
            out
        };
        // Answer jobs on this thread's behalf.
        let answering = std::thread::spawn(move || {
            if let Ok(job) = remote.jobs.recv_timeout(Duration::from_secs(3)) {
                let _ = job
                    .reply
                    .send((200, format!("{{\"path\":{:?}}}", job.request.path)));
            }
            remote
        });
        assert!(ask("/api/state").starts_with("HTTP/1.1 401"));
        let ok = ask("/api/state?token=tok");
        assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
        assert!(ok.contains("\"path\""));
        let _remote = answering.join().unwrap();
    }
}
