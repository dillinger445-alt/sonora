//! Web requests through the system's `curl` (part of macOS and of Windows 10
//! and later), so SONORA itself contains no network code. Used to talk to
//! slskd, the Soulseek client the person runs on their own computer.
//!
//! The request (address, headers, body) goes to curl through its standard
//! input, never its command line, where other programs on the computer could
//! read it. curl's own configuration files are ignored, so nothing the
//! person's `.curlrc` sets (headers, cookies, tracing) is added.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

/// Largest answer read (a search with many results is a few megabytes).
const MAX_BODY: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct Request {
    pub method: &'static str,
    pub url: String,
    pub headers: Vec<(String, String)>,
    /// A JSON body.
    pub body: Option<String>,
    pub timeout: Duration,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Response {
    pub status: u16,
    pub body: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum NetError {
    /// curl could not be started.
    NoCurl(String),
    /// Nothing answers at that address (refused, unknown host, no route).
    Unreachable(String),
    TimedOut,
    Failed(String),
}

/// How requests leave SONORA (replaced by a fake in tests).
pub trait Http: Send + Sync {
    fn send(&self, req: &Request) -> Result<Response, NetError>;
}

pub struct Curl {
    program: PathBuf,
}

impl Default for Curl {
    fn default() -> Self {
        let mut program = PathBuf::from("curl");
        if cfg!(windows) {
            if let Some(root) = std::env::var_os("SystemRoot") {
                let p = PathBuf::from(root).join("System32").join("curl.exe");
                if p.is_file() {
                    program = p;
                }
            }
        } else if PathBuf::from("/usr/bin/curl").is_file() {
            program = PathBuf::from("/usr/bin/curl");
        }
        Curl { program }
    }
}

/// Quotes a value for a curl configuration file.
fn quote(v: &str) -> String {
    format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\"").replace(['\n', '\r'], " "))
}

/// The configuration curl reads for a request.
fn config(req: &Request) -> String {
    let mut c = format!("url = {}\nrequest = {}\n", quote(&req.url), quote(req.method));
    for (k, v) in &req.headers {
        c += &format!("header = {}\n", quote(&format!("{k}: {v}")));
    }
    if let Some(body) = &req.body {
        c += &format!("header = {}\n", quote("Content-Type: application/json"));
        c += &format!("data-binary = {}\n", quote(body));
    }
    c += "write-out = \"\\n%{http_code}\"\n";
    c
}

/// Splits curl's output into the body and the status code written after it.
fn split_status(out: &str) -> Option<Response> {
    let (body, code) = out.rsplit_once('\n').unwrap_or(("", out));
    let status = code.trim().parse::<u16>().ok()?;
    Some(Response { status, body: body.to_string() })
}

/// What a failed curl run means, from its error line ("curl: (7) Failed to connect…").
fn classify(stderr: &str) -> NetError {
    let line = stderr.lines().rev().find(|l| l.starts_with("curl: (")).map(|l| l.trim_start_matches("curl: ").to_string());
    let Some(line) = line else { return NetError::Failed("the request failed".into()) };
    let code = line.trim_start_matches('(').split(')').next().and_then(|c| c.parse::<u32>().ok());
    match code {
        // Could not resolve the host, could not connect.
        Some(6) | Some(7) => NetError::Unreachable(line),
        Some(28) => NetError::TimedOut,
        _ => NetError::Failed(line),
    }
}

impl Http for Curl {
    fn send(&self, req: &Request) -> Result<Response, NetError> {
        let secs = req.timeout.as_secs().max(1);
        let mut cmd = sonora_engine::decode::tools::quiet_command(&self.program);
        // `-q` first: ignore the person's .curlrc. `--noproxy *`: talk to the
        // address directly, never through a proxy set in the environment.
        cmd.args(["-q", "--silent", "--show-error", "--proto", "=http,https", "--noproxy", "*", "--max-time", &secs.to_string(), "--config", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| NetError::NoCurl(format!("curl could not be started ({e}); it is part of macOS and Windows 10 and later")))?;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(config(req).as_bytes());
        }
        let out = child.stdout.take().ok_or_else(|| NetError::Failed("no output from curl".into()))?;
        let mut err = child.stderr.take().ok_or_else(|| NetError::Failed("no output from curl".into()))?;
        let reader = std::thread::spawn(move || {
            let mut b = Vec::new();
            let _ = out.take(MAX_BODY).read_to_end(&mut b);
            b
        });
        let err_reader = std::thread::spawn(move || {
            let mut b = Vec::new();
            let _ = (&mut err).take(64 * 1024).read_to_end(&mut b);
            b
        });
        let started = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break s,
                Ok(None) if started.elapsed() > req.timeout + Duration::from_secs(5) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(NetError::TimedOut);
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(e) => return Err(NetError::Failed(e.to_string())),
            }
        };
        let body = String::from_utf8_lossy(&reader.join().unwrap_or_default()).to_string();
        let err = String::from_utf8_lossy(&err_reader.join().unwrap_or_default()).trim().to_string();
        if !status.success() {
            return Err(classify(&err));
        }
        split_status(&body).ok_or_else(|| NetError::Failed("unexpected answer from curl".into()))
    }
}

/// Percent-encodes one part of a URL path (a user name, say).
pub fn path_segment(s: &str) -> String {
    let mut o = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => o.push(b as char),
            _ => o.push_str(&format!("%{b:02X}")),
        }
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_become_curl_configuration() {
        let req = Request {
            method: "POST",
            url: "http://localhost:5030/api/v0/searches".into(),
            headers: vec![("X-API-Key".into(), "k\"ey".into())],
            body: Some(r#"{"searchText":"a \"b\" c:\\d"}"#.into()),
            timeout: Duration::from_secs(5),
        };
        let c = config(&req);
        assert!(c.contains("url = \"http://localhost:5030/api/v0/searches\"\n"), "{c}");
        assert!(c.contains("request = \"POST\"\n"));
        assert!(c.contains("header = \"X-API-Key: k\\\"ey\"\n"), "{c}");
        assert!(c.contains("header = \"Content-Type: application/json\"\n"));
        assert!(c.contains(r#"data-binary = "{\"searchText\":\"a \\\"b\\\" c:\\\\d\"}""#), "{c}");
    }

    #[test]
    fn status_and_errors_are_read_from_curl() {
        assert_eq!(split_status("{\"a\":1}\n200"), Some(Response { status: 200, body: "{\"a\":1}".into() }));
        assert_eq!(split_status("\n204"), Some(Response { status: 204, body: String::new() }));
        assert_eq!(split_status("garbage"), None);
        assert!(matches!(classify("curl: (7) Failed to connect to localhost port 5030"), NetError::Unreachable(_)));
        assert!(matches!(classify("curl: (6) Could not resolve host: nas"), NetError::Unreachable(_)));
        assert_eq!(classify("curl: (28) Operation timed out after 15001 milliseconds"), NetError::TimedOut);
        assert!(matches!(classify("curl: (60) SSL certificate problem"), NetError::Failed(_)));
        assert!(matches!(classify(""), NetError::Failed(_)));
    }

    #[test]
    fn path_segments_are_encoded() {
        assert_eq!(path_segment("DJ Some/One"), "DJ%20Some%2FOne");
        assert_eq!(path_segment("plain-name_1.~"), "plain-name_1.~");
        assert_eq!(path_segment("Ünïcode"), "%C3%9Cn%C3%AFcode");
    }
}
