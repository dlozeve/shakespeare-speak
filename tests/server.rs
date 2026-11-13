//! End-to-end checks of the HTTP server.
//!
//! Each test binds its own port so they can run in parallel, and the guard kills
//! the child server when the test finishes.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

struct ServerGuard {
    child: Child,
    addr: String,
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Start a server on `port` and wait until it accepts connections.
fn start(port: u16) -> ServerGuard {
    let addr = format!("127.0.0.1:{port}");

    let child = Command::new(env!("CARGO_BIN_EXE_shakespeare-speak"))
        .args(["--serve", "--bind", &addr])
        .env_remove("OPENROUTER_API_KEY")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn server");

    let guard = ServerGuard { child, addr };

    for _ in 0..200 {
        if TcpStream::connect(&guard.addr).is_ok() {
            return guard;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("server did not start on {}", guard.addr);
}

/// Minimal HTTP/1.1 request. Returns (status, body).
fn request(addr: &str, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
    let mut stream = TcpStream::connect(addr).expect("connect");
    let payload = body.unwrap_or("");

    let head = format!(
        "{method} {path} HTTP/1.1\r\n\
         Host: {addr}\r\n\
         Connection: close\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\r\n",
        payload.len()
    );

    stream.write_all(head.as_bytes()).expect("write head");
    stream.write_all(payload.as_bytes()).expect("write body");

    let mut raw = String::new();
    stream.read_to_string(&mut raw).expect("read response");

    let (headers, body) = raw.split_once("\r\n\r\n").unwrap_or((raw.as_str(), ""));
    let status = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);

    (status, body.to_string())
}

fn json(body: &str) -> serde_json::Value {
    serde_json::from_str(body).unwrap_or_else(|error| panic!("bad JSON ({error}): {body}"))
}

#[test]
fn health_reports_the_corpus() {
    let server = start(18740);
    let (status, body) = request(&server.addr, "GET", "/health", None);

    assert_eq!(status, 200);
    let value = json(&body);
    assert_eq!(value["status"], "ok");
    assert_eq!(value["works"], 44);
}

#[test]
fn search_returns_verified_json_without_an_api_key() {
    let server = start(18741);
    let (status, body) = request(
        &server.addr,
        "GET",
        "/search?q=at+one+fell+swoop&verses=2",
        None,
    );

    assert_eq!(status, 200);
    let value = json(&body);
    let hit = &value["hits"][0];

    assert_eq!(hit["work_slug"], "the-tragedy-of-macbeth");
    assert_eq!(hit["act"], 4);
    assert_eq!(hit["scene_roman"], "iii");
    // Verse line breaks survive the round trip.
    assert_eq!(
        hit["quote"],
        "What, all my pretty chickens and their dam\nAt one fell swoop?"
    );
}

#[test]
fn quote_without_a_key_is_unavailable() {
    let server = start(18742);
    let (status, body) = request(&server.addr, "GET", "/quote?q=in+one+go", None);

    assert_eq!(status, 503);
    assert!(json(&body)["error"]
        .as_str()
        .unwrap()
        .contains("OPENROUTER_API_KEY"));
}

#[test]
fn missing_query_is_a_bad_request() {
    let server = start(18743);

    let (status, _) = request(&server.addr, "GET", "/quote", None);
    assert_eq!(status, 400);

    let (status, _) = request(&server.addr, "GET", "/search", None);
    assert_eq!(status, 400);
}

#[test]
fn unknown_paths_are_not_found() {
    let server = start(18744);
    let (status, body) = request(&server.addr, "GET", "/nope", None);

    assert_eq!(status, 404);
    assert_eq!(json(&body)["error"], "not found");
}

#[test]
fn numeric_parameters_are_clamped() {
    let server = start(18745);
    // verses=99 must not blow up; it clamps to the maximum.
    let (status, body) = request(
        &server.addr,
        "GET",
        "/search?q=at+one+fell+swoop&verses=99",
        None,
    );

    assert_eq!(status, 200);
    let value = json(&body);
    assert_eq!(value["total"], 1);
}
