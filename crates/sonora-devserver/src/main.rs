//! `sonora-devserver` — the browser bridge.
//!
//! Serves the built interface (`--static dist`) and exposes the same core
//! API the desktop app uses over Tauri IPC:
//!
//! - `POST /api/call` with `{"method": "...", "params": {...}}` →
//!   `{"ok": true, "result": ...}` or `{"ok": false, "error": {code, message}}`
//! - `GET /api/events` → Server-Sent Events (`event: <name>`, `data: <json>`)
//! - `GET /api/audio?path=…` → a file the core prepared for playback
//!   (`playback.prepare`), with HTTP range support for seeking. Other paths
//!   are refused, like the desktop app's private `sonora:` scheme.
//!
//! Development only: it listens on 127.0.0.1, rejects cross-origin and
//! foreign-Host requests, and is never part of the desktop build.
//!
//! ```text
//! sonora-devserver [--port 4317] [--static DIR] [--data DIR]
//! ```

use sonora_core::{App, AppConfig};
use sonora_json::{obj, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Files the core prepared for playback; the only ones `/api/audio` serves.
type Playable = Arc<Mutex<HashSet<PathBuf>>>;

const MAX_BODY: usize = 16 * 1024 * 1024;

struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

fn read_request(stream: &TcpStream) -> std::io::Result<Option<Request>> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("/").to_string();
    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h)? == 0 || h == "\r\n" || h == "\n" {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
        if headers.len() > 100 {
            break;
        }
    }
    let len: usize = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-length")).and_then(|(_, v)| v.parse().ok()).unwrap_or(0);
    if len > MAX_BODY {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "body too large"));
    }
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body)?;
    Ok(Some(Request { method, path, headers, body }))
}

fn respond(stream: &mut TcpStream, status: &str, content_type: &str, body: &[u8], extra: &str) {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n{extra}\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
}

fn content_type(p: &Path) -> &'static str {
    match p.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "map" => "application/json",
        _ => "application/octet-stream",
    }
}

/// Accept only requests addressed to this machine and, when an Origin is
/// sent, only same-origin ones (blocks cross-site requests and DNS rebinding).
fn trusted(req: &Request, port: u16) -> bool {
    let host_ok = req.header("host").is_some_and(|h| {
        let h = h.to_ascii_lowercase();
        let name = h.rsplit_once(':').map(|(n, _)| n.to_string()).unwrap_or(h.clone());
        name == "localhost" || name == "127.0.0.1" || name == "[::1]"
    });
    let origin_ok = match req.header("origin") {
        None => true,
        Some(o) => {
            let o = o.to_ascii_lowercase();
            // Same origin as us, or the Vite dev server proxying to us.
            o == format!("http://localhost:{port}") || o == format!("http://127.0.0.1:{port}") || o == "http://localhost:1420" || o == "http://127.0.0.1:1420"
        }
    };
    host_ok && origin_ok
}

fn serve_static(stream: &mut TcpStream, root: &Path, url_path: &str) {
    let clean = url_path.split(['?', '#']).next().unwrap_or("/");
    let rel = clean.trim_start_matches('/');
    let rel = if rel.is_empty() { "index.html" } else { rel };
    if rel.split('/').any(|seg| seg == ".." || seg.contains('\\')) {
        respond(stream, "400 Bad Request", "text/plain", b"bad path", "");
        return;
    }
    let mut file = root.join(rel);
    let canon_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    if !file.is_file() {
        // Single-page app fallback.
        file = root.join("index.html");
    }
    match file.canonicalize() {
        Ok(f) if f.starts_with(&canon_root) => match std::fs::read(&f) {
            Ok(bytes) => respond(stream, "200 OK", content_type(&f), &bytes, ""),
            Err(_) => respond(stream, "404 Not Found", "text/plain", b"not found", ""),
        },
        _ => respond(stream, "404 Not Found", "text/plain", b"not found", ""),
    }
}

fn handle_call(stream: &mut TcpStream, app: &Arc<App>, req: &Request, playable: &Playable) {
    if req.header("x-sonora-client").is_none() {
        respond(stream, "403 Forbidden", "text/plain", b"missing X-Sonora-Client header", "");
        return;
    }
    let body = String::from_utf8_lossy(&req.body);
    let parsed = match sonora_json::parse(&body) {
        Ok(v) => v,
        Err(e) => {
            let out = obj! { "ok" => false, "error" => obj! { "code" => "invalid_params", "message" => format!("request is not valid JSON: {e}") } };
            respond(stream, "200 OK", "application/json", out.to_string_compact().as_bytes(), "");
            return;
        }
    };
    let method = parsed.get("method").and_then(Value::as_str).unwrap_or("").to_string();
    let params = parsed.get("params").cloned().unwrap_or(Value::object());
    let out = match app.call(&method, &params.to_string_compact()) {
        Ok(json) => {
            if method == "playback.prepare" {
                if let Some(path) = sonora_json::parse(&json).ok().and_then(|v| v.get("path").and_then(Value::as_str).map(PathBuf::from)) {
                    if let Ok(mut set) = playable.lock() {
                        set.insert(path);
                    }
                }
            }
            format!("{{\"ok\":true,\"result\":{json}}}")
        }
        Err(e) => obj! { "ok" => false, "error" => obj! { "code" => e.code, "message" => e.message } }.to_string_compact(),
    };
    respond(stream, "200 OK", "application/json", out.as_bytes(), "");
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn audio_type(p: &Path) -> &'static str {
    match p.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("mp3") => "audio/mpeg",
        Some("m4a") | Some("mp4") | Some("aac") => "audio/mp4",
        Some("flac") => "audio/flac",
        Some("ogg") | Some("oga") | Some("opus") => "audio/ogg",
        Some("aif") | Some("aiff") | Some("aifc") => "audio/aiff",
        Some("wav") | Some("wave") => "audio/wav",
        _ => "application/octet-stream",
    }
}

/// Parses a single `bytes=` range against a length: (start, end inclusive).
fn parse_range(h: &str, len: u64) -> Option<(u64, u64)> {
    let spec = h.trim().strip_prefix("bytes=")?;
    if spec.contains(',') || len == 0 {
        return None;
    }
    let (a, b) = spec.split_once('-')?;
    let (start, end) = if a.trim().is_empty() {
        let n: u64 = b.trim().parse().ok()?;
        (len.saturating_sub(n), len - 1)
    } else {
        let start: u64 = a.trim().parse().ok()?;
        let end = if b.trim().is_empty() { len - 1 } else { b.trim().parse::<u64>().ok()?.min(len - 1) };
        (start, end)
    };
    (start <= end && start < len).then_some((start, end))
}

fn handle_audio(stream: &mut TcpStream, req: &Request, playable: &Playable) {
    let query = req.path.split_once('?').map(|(_, q)| q).unwrap_or("");
    let path = query.split('&').find_map(|kv| kv.strip_prefix("path=")).map(percent_decode).map(PathBuf::from);
    let allowed = path.as_ref().is_some_and(|p| playable.lock().map(|s| s.contains(p)).unwrap_or(false));
    let Some(path) = path.filter(|_| allowed) else {
        respond(stream, "403 Forbidden", "text/plain", b"not prepared for playback", "");
        return;
    };
    let Ok(mut file) = std::fs::File::open(&path) else {
        respond(stream, "404 Not Found", "text/plain", b"not found", "");
        return;
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let kind = audio_type(&path);
    let (status, start, end) = match req.header("range") {
        Some(h) => match parse_range(h, len) {
            Some((a, b)) => ("206 Partial Content", a, b),
            None => {
                respond(stream, "416 Range Not Satisfiable", "text/plain", b"", &format!("Content-Range: bytes */{len}\r\n"));
                return;
            }
        },
        None => ("200 OK", 0, len.saturating_sub(1)),
    };
    let count = if len == 0 { 0 } else { end - start + 1 };
    let range = if status.starts_with("206") { format!("Content-Range: bytes {start}-{end}/{len}\r\n") } else { String::new() };
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {count}\r\nAccept-Ranges: bytes\r\n{range}Connection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n\r\n"
    );
    if stream.write_all(head.as_bytes()).is_err() {
        return;
    }
    use std::io::Seek;
    if file.seek(std::io::SeekFrom::Start(start)).is_ok() {
        // The browser closes the connection once it has enough; that is not an error.
        let _ = std::io::copy(&mut file.take(count), stream);
    }
}

fn handle_events(mut stream: TcpStream, app: &Arc<App>) {
    let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-store\r\nConnection: keep-alive\r\nX-Accel-Buffering: no\r\n\r\n";
    if stream.write_all(head.as_bytes()).is_err() {
        return;
    }
    let (tx, rx) = mpsc::sync_channel::<String>(512);
    let id = app.subscribe(Box::new(move |name, payload| {
        // Never block the core: drop events for a slow or dead client.
        let _ = tx.try_send(format!("event: {name}\ndata: {payload}\n\n"));
    }));
    let _ = stream.write_all(b": connected\n\n");
    loop {
        match rx.recv_timeout(Duration::from_secs(15)) {
            Ok(msg) => {
                if stream.write_all(msg.as_bytes()).is_err() {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if stream.write_all(b": ping\n\n").is_err() {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        let _ = stream.flush();
    }
    app.unsubscribe(id);
}

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("sonora-devserver [--port 4317] [--static DIR] [--data DIR]");
        return;
    }
    let port: u16 = arg(&args, "--port").and_then(|p| p.parse().ok()).unwrap_or(4317);
    let static_dir = arg(&args, "--static").map(PathBuf::from);
    let data_dir = arg(&args, "--data")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("SONORA_DATA_DIR").map(PathBuf::from))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default().join(".sonora-dev-data"));
    let app = match App::open(AppConfig { data_dir: data_dir.clone(), tool_dirs: vec![], app_version: format!("{}-dev", env!("CARGO_PKG_VERSION")) }) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("could not start the core: {e}");
            std::process::exit(1);
        }
    };
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("could not listen on 127.0.0.1:{port}: {e}");
            std::process::exit(1);
        }
    };
    eprintln!("SONORA dev bridge on http://127.0.0.1:{port}  (data: {})", data_dir.display());
    if let Some(s) = &static_dir {
        eprintln!("serving interface from {}", s.display());
    }
    let playable: Playable = Arc::new(Mutex::new(HashSet::new()));
    for conn in listener.incoming() {
        let Ok(mut stream) = conn else { continue };
        let app = Arc::clone(&app);
        let static_dir = static_dir.clone();
        let playable = Arc::clone(&playable);
        std::thread::spawn(move || {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
            let req = match read_request(&stream) {
                Ok(Some(r)) => r,
                _ => return,
            };
            if !trusted(&req, port) {
                respond(&mut stream, "403 Forbidden", "text/plain", b"forbidden", "");
                return;
            }
            let route = req.path.split('?').next().unwrap_or("").to_string();
            match (req.method.as_str(), route.as_str()) {
                ("POST", "/api/call") => handle_call(&mut stream, &app, &req, &playable),
                ("GET", "/api/audio") => {
                    let _ = stream.set_read_timeout(None);
                    handle_audio(&mut stream, &req, &playable)
                }
                ("GET", "/api/events") => {
                    let _ = stream.set_read_timeout(None);
                    handle_events(stream, &app)
                }
                ("GET", "/api/health") => respond(&mut stream, "200 OK", "application/json", b"{\"ok\":true}", ""),
                ("GET", _) => match &static_dir {
                    Some(root) => serve_static(&mut stream, root, &req.path),
                    None => respond(&mut stream, "404 Not Found", "text/plain", b"no --static directory configured", ""),
                },
                _ => respond(&mut stream, "405 Method Not Allowed", "text/plain", b"method not allowed", ""),
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=0-", 100), Some((0, 99)));
        assert_eq!(parse_range("bytes=10-19", 100), Some((10, 19)));
        assert_eq!(parse_range("bytes=90-200", 100), Some((90, 99)));
        assert_eq!(parse_range("bytes=-10", 100), Some((90, 99)));
        assert_eq!(parse_range("bytes=100-", 100), None);
        assert_eq!(parse_range("bytes=5-1", 100), None);
        assert_eq!(parse_range("bytes=0-1,5-6", 100), None);
        assert_eq!(parse_range("items=0-1", 100), None);
    }

    #[test]
    fn decoding() {
        assert_eq!(percent_decode("%2Fmusic%2FA%20B%2B.flac"), "/music/A B+.flac");
        assert_eq!(percent_decode("%C3%91"), "Ñ");
        assert_eq!(percent_decode("bad%2"), "bad%2");
        assert_eq!(percent_decode("%aÑ"), "%aÑ", "no panic inside a multi-byte character");
    }
}
