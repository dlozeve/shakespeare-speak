//! A basic HTTP front end over the same pipeline the CLI uses.
//!
//! The server never invents anything either: `/quote` runs the identical
//! propose → verify → rank path, and `/search` is a plain corpus lookup that
//! needs no API key at all.

use std::sync::Arc;

use anyhow::{anyhow, Result};
use serde_json::json;
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

use crate::corpus::Corpus;
use crate::llm;
use crate::{find_answer, render_json, search_json, Options, Outcome, Params};

/// Number of worker threads reading from the accept queue.
const WORKERS: usize = 4;

pub fn serve(
    corpus: Arc<Corpus>,
    client: Option<Arc<llm::Client>>,
    bind: &str,
    defaults: Options,
) -> Result<()> {
    let server =
        Arc::new(Server::http(bind).map_err(|error| anyhow!("cannot bind {bind}: {error}"))?);

    eprintln!("shakespeare-speak listening on http://{bind}");
    eprintln!("  GET  /health");
    eprintln!("  GET  /quote?q=<phrase>[&verses=1-4][&options=1-10][&candidates=N][&context=N]");
    eprintln!("  POST /quote            {{\"query\": \"<phrase>\"}}");
    eprintln!("  GET  /search?q=<phrase>[&verses=1-4][&limit=N]");
    if client.is_none() {
        eprintln!("  note: /quote will answer 503 until OPENROUTER_API_KEY is set");
    }

    let mut workers = Vec::with_capacity(WORKERS);
    for _ in 0..WORKERS {
        let server = Arc::clone(&server);
        let corpus = Arc::clone(&corpus);
        let client = client.clone();
        let defaults = defaults.clone();

        workers.push(std::thread::spawn(move || {
            for request in server.incoming_requests() {
                if let Err(error) = handle(request, &corpus, client.as_deref(), &defaults) {
                    eprintln!("request error: {error:#}");
                }
            }
        }));
    }

    for worker in workers {
        let _ = worker.join();
    }
    Ok(())
}

fn handle(
    request: Request,
    corpus: &Corpus,
    client: Option<&llm::Client>,
    defaults: &Options,
) -> Result<()> {
    let url = request.url().to_string();
    let (path, query) = url.split_once('?').unwrap_or((url.as_str(), ""));
    let params = parse_query(query);

    let is_get = *request.method() == Method::Get;
    let is_post = *request.method() == Method::Post;
    let is_options = *request.method() == Method::Options;

    if is_options {
        return preflight(request);
    }

    match (is_get, is_post, path) {
        (true, _, "/health") => respond(
            request,
            200,
            json!({
                "status": "ok",
                "works": corpus.works.len(),
                "corpus": corpus.source.as_str(),
                "model": client.map(|client| client.model.clone()),
            }),
        ),

        (true, _, "/search") => {
            let Some(phrase) = params.get("q") else {
                return bad_request(request, "missing ?q=<phrase>");
            };
            let verses = number(&params, "verses", defaults.verses, 1, 4);
            let limit = number(&params, "limit", 20, 1, 200);
            respond(request, 200, search_json(corpus, phrase, verses, limit))
        }

        (true, _, "/quote") => {
            let Some(phrase) = params.get("q").cloned() else {
                return bad_request(request, "missing ?q=<phrase>");
            };
            quote(request, corpus, client, defaults, &phrase, &params)
        }

        (_, true, "/quote") => {
            let mut request = request;
            let mut body = String::new();
            request.as_reader().read_to_string(&mut body)?;

            let parsed: serde_json::Value = match serde_json::from_str(&body) {
                Ok(value) => value,
                Err(error) => {
                    return bad_request(request, &format!("body is not JSON: {error}"));
                }
            };

            let Some(phrase) = parsed.get("query").and_then(|value| value.as_str()) else {
                return bad_request(request, "missing \"query\" in the JSON body");
            };
            let phrase = phrase.to_string();
            quote(request, corpus, client, defaults, &phrase, &params)
        }

        _ => respond(
            request,
            404,
            json!({
                "error": "not found",
                "endpoints": ["/health", "/quote?q=", "/search?q="],
            }),
        ),
    }
}

fn quote(
    request: Request,
    corpus: &Corpus,
    client: Option<&llm::Client>,
    defaults: &Options,
    phrase: &str,
    params: &Params,
) -> Result<()> {
    let Some(client) = client else {
        return respond(
            request,
            503,
            json!({
                "error": "OPENROUTER_API_KEY is not set; /quote is unavailable. Use /search.",
            }),
        );
    };

    let opts = Options {
        verses: number(params, "verses", defaults.verses, 1, 4),
        options: number(params, "options", defaults.options, 1, 10),
        candidates: number(params, "candidates", defaults.candidates, 1, 30),
        context: number(params, "context", defaults.context, 0, 10),
        semantic_check: defaults.semantic_check,
    };

    match find_answer(corpus, client, phrase, &opts) {
        Ok(outcome) => {
            let status = match outcome {
                Outcome::Found(_) => 200,
                Outcome::NotFound { .. } => 404,
            };
            respond(request, status, render_json(phrase, &outcome, &opts))
        }
        Err(error) => respond(
            request,
            502,
            json!({ "error": format!("{error:#}"), "query": phrase }),
        ),
    }
}

fn bad_request(request: Request, message: &str) -> Result<()> {
    respond(request, 400, json!({ "error": message }))
}

fn respond(request: Request, status: u16, body: serde_json::Value) -> Result<()> {
    let text = serde_json::to_string_pretty(&body)?;
    let bytes = text.into_bytes();
    let length = bytes.len();

    let response = Response::new(
        StatusCode(status),
        vec![
            header("Content-Type", "application/json; charset=utf-8"),
            header("Access-Control-Allow-Origin", "*"),
            header("Cache-Control", "no-store"),
        ],
        std::io::Cursor::new(bytes),
        Some(length),
        None,
    );

    request.respond(response)?;
    Ok(())
}

fn preflight(request: Request) -> Result<()> {
    let response = Response::empty(StatusCode(204))
        .with_header(header("Access-Control-Allow-Methods", "GET, POST, OPTIONS"));
    request.respond(response)?;
    Ok(())
}

/// Build a header. The name and value are ASCII constants, so this cannot fail.
fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes())
        .expect("header name and value are static ASCII")
}

/// Parse `a=1&b=2`, percent-decoding keys and values.
fn parse_query(query: &str) -> Params {
    let mut params = Params::new();
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        params.insert(percent_decode(key), percent_decode(value));
    }
    params
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            b'%' if index + 3 <= bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok();
                match hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                    Some(byte) => {
                        out.push(byte);
                        index += 3;
                    }
                    None => {
                        out.push(bytes[index]);
                        index += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }

    String::from_utf8_lossy(&out).into_owned()
}

/// Read a numeric parameter, clamped to `min..=max`.
fn number(params: &Params, key: &str, default: usize, min: usize, max: usize) -> usize {
    params
        .get(key)
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(default)
        .clamp(min, max)
}

#[cfg(test)]
mod tests {
    use super::{number, parse_query, percent_decode};

    #[test]
    fn decodes_query_strings() {
        let params = parse_query("q=in+one+go&verses=4");
        assert_eq!(params.get("q").map(String::as_str), Some("in one go"));
        assert_eq!(params.get("verses").map(String::as_str), Some("4"));
    }

    #[test]
    fn decodes_percent_escapes() {
        assert_eq!(percent_decode("caf%C3%A9"), "café");
        assert_eq!(percent_decode("a%20b"), "a b");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
    }

    #[test]
    fn clamps_numbers() {
        let params = parse_query("verses=99&options=0&bogus=3&junk=abc");
        // Present and in range: parsed as-is.
        assert_eq!(number(&params, "bogus", 7, 1, 10), 3);
        // Present but out of range: clamped.
        assert_eq!(number(&params, "verses", 2, 1, 4), 4);
        assert_eq!(number(&params, "options", 3, 1, 10), 1);
        // Present but unparseable, or absent: the default.
        assert_eq!(number(&params, "junk", 6, 1, 10), 6);
        assert_eq!(number(&params, "missing", 5, 1, 10), 5);
    }
}
