//! The little of HTTP the artifact server speaks: reading a request and
//! writing an answer, over a blocking stream.
//!
//! It answers browsers on this computer only, and serves files and a few
//! small JSON calls, so it is written here rather than taken from a web
//! framework: no async runtime, and nothing to keep patched but this.

use std::fs::File;
use std::io::{BufRead, Read, Seek as _, SeekFrom, Write};
use std::path::Path;

/// The request line and headers together.
const MAX_HEAD: usize = 16 * 1024;

#[derive(Debug)]
pub struct Request {
    pub method: String,
    /// The path as it reads (`%20` a space again), without the query.
    pub path: String,
    query: String,
    /// Names in lowercase.
    headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// HTTP/1.1, where a connection stays open unless either side says not.
    pub keeps_alive: bool,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(known, _)| known == name)
            .map(|(_, value)| value.as_str())
    }

    /// The value of `name` in the query (`?key=todos`).
    pub fn query(&self, name: &str) -> Option<String> {
        self.query.split('&').find_map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (key == name).then(|| percent_decode(value)).flatten()
        })
    }
}

#[derive(Debug, PartialEq)]
pub enum ReadError {
    /// The other side hung up, or sent nothing for too long.
    Closed,
    /// Not a request this server reads.
    Bad(&'static str),
    /// A body larger than `max_body`.
    TooLarge,
}

/// Reads one request; a body of at most `max_body` bytes.
pub fn read_request(reader: &mut impl BufRead, max_body: usize) -> Result<Request, ReadError> {
    let mut read = 0;
    let mut line = |reader: &mut dyn BufRead| -> Result<String, ReadError> {
        let mut text = Vec::new();
        let taken = reader
            .take((MAX_HEAD - read) as u64 + 1)
            .read_until(b'\n', &mut text)
            .map_err(|_| ReadError::Closed)?;
        read += taken;
        if taken == 0 {
            return Err(ReadError::Closed);
        }
        if read > MAX_HEAD || text.last() != Some(&b'\n') {
            return Err(ReadError::Bad("the request's headers are too long"));
        }
        String::from_utf8(text)
            .map(|text| text.trim_end_matches(['\r', '\n']).to_string())
            .map_err(|_| ReadError::Bad("the request is not text"))
    };

    let first = line(reader)?;
    let mut parts = first.split(' ');
    let (Some(method), Some(target), Some(version)) = (parts.next(), parts.next(), parts.next())
    else {
        return Err(ReadError::Bad("not an HTTP request"));
    };
    if !version.starts_with("HTTP/1.") || !target.starts_with('/') {
        return Err(ReadError::Bad("not an HTTP/1 request for a path"));
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let path = percent_decode(path).ok_or(ReadError::Bad("the path is not UTF-8"))?;

    let mut headers = Vec::new();
    loop {
        let header = line(reader)?;
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
    }
    let header = |name: &str| {
        headers
            .iter()
            .find(|(known, _)| known == name)
            .map(|(_, value)| value.as_str())
    };
    // Browsers give a length with what they send here; a body in chunks is
    // some other client's.
    if header("transfer-encoding").is_some() {
        return Err(ReadError::Bad("send the body with a Content-Length"));
    }
    let length: usize = match header("content-length") {
        Some(length) => length
            .parse()
            .map_err(|_| ReadError::Bad("Content-Length is not a number"))?,
        None => 0,
    };
    if length > max_body {
        return Err(ReadError::TooLarge);
    }
    let mut body = vec![0; length];
    reader
        .read_exact(&mut body)
        .map_err(|_| ReadError::Closed)?;
    let keeps_alive = version == "HTTP/1.1"
        && !header("connection").is_some_and(|value| value.eq_ignore_ascii_case("close"));
    Ok(Request {
        method: method.to_string(),
        path,
        query: query.to_string(),
        headers,
        body,
        keeps_alive,
    })
}

/// `text` with its `%XX` read back; `None` when that is not UTF-8.
pub fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut ix = 0;
    while ix < bytes.len() {
        let hex = (bytes[ix] == b'%')
            .then(|| bytes.get(ix + 1..ix + 3))
            .flatten()
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match hex {
            Some(byte) => {
                out.push(byte);
                ix += 3;
            }
            None => {
                out.push(bytes[ix]);
                ix += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

pub enum Body {
    Empty,
    Bytes(Vec<u8>),
    /// `len` bytes of a file from `start`.
    File {
        file: File,
        start: u64,
        len: u64,
    },
}

pub struct Response {
    pub status: u16,
    headers: Vec<(&'static str, String)>,
    body: Body,
}

impl Response {
    pub fn new(status: u16) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: Body::Empty,
        }
    }

    /// A few words for a person, or for an agent reading the page.
    pub fn text(status: u16, text: &str) -> Self {
        Self::new(status)
            .header("Content-Type", "text/plain; charset=utf-8")
            .body(Body::Bytes(text.as_bytes().to_vec()))
    }

    pub fn json(status: u16, json: Vec<u8>) -> Self {
        Self::new(status)
            .header("Content-Type", "application/json")
            .header("Cache-Control", "no-store")
            .body(Body::Bytes(json))
    }

    /// What went wrong, as a script reads it: `{"error": "…"}`.
    pub fn error(status: u16, message: &str) -> Self {
        Self::json(
            status,
            serde_json::json!({ "error": message })
                .to_string()
                .into_bytes(),
        )
    }

    pub fn header(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.headers.push((name, value.into()));
        self
    }

    pub fn body(mut self, body: Body) -> Self {
        self.body = body;
        self
    }

    /// Writes the answer; without its body for a `HEAD`.
    pub fn write(
        self,
        out: &mut impl Write,
        head_only: bool,
        keep_alive: bool,
    ) -> std::io::Result<()> {
        let length = match &self.body {
            Body::Empty => 0,
            Body::Bytes(bytes) => bytes.len() as u64,
            Body::File { len, .. } => *len,
        };
        let mut head = format!(
            "HTTP/1.1 {} {}\r\nContent-Length: {length}\r\nConnection: {}\r\n",
            self.status,
            reason(self.status),
            if keep_alive { "keep-alive" } else { "close" }
        );
        for (name, value) in &self.headers {
            // A header's value never carries a line of its own.
            let value = value.replace(['\r', '\n'], " ");
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str("\r\n");
        out.write_all(head.as_bytes())?;
        if !head_only {
            match self.body {
                Body::Empty => {}
                Body::Bytes(bytes) => out.write_all(&bytes)?,
                Body::File {
                    mut file,
                    start,
                    len,
                } => {
                    file.seek(SeekFrom::Start(start))?;
                    std::io::copy(&mut file.take(len), out)?;
                }
            }
        }
        out.flush()
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        206 => "Partial Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Content Too Large",
        416 => "Range Not Satisfiable",
        503 => "Service Unavailable",
        _ => "Error",
    }
}

/// The bytes `header` (`Range: bytes=0-99`) asks for of a file `len` long:
/// first and last, both counted. `None` for anything but one plain range,
/// which is then answered with the whole file.
pub fn range(header: &str, len: u64) -> Option<(u64, u64)> {
    let spec = header.trim().strip_prefix("bytes=")?;
    let (first, last) = spec.split_once('-')?;
    if spec.contains(',') || len == 0 {
        return None;
    }
    let (first, last) = match (first.trim(), last.trim()) {
        // The last N bytes.
        ("", last) => (len.saturating_sub(last.parse().ok()?), len - 1),
        (first, "") => (first.parse().ok()?, len - 1),
        (first, last) => (first.parse().ok()?, last.parse::<u64>().ok()?.min(len - 1)),
    };
    (first <= last && first < len).then_some((first, last))
}

/// What a browser should take the file at `path` for, by its name.
pub fn content_type(path: &Path) -> &'static str {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" | "cjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json",
        "webmanifest" => "application/manifest+json",
        "txt" | "md" => "text/plain; charset=utf-8",
        "csv" => "text/csv; charset=utf-8",
        "xml" => "application/xml",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "bmp" => "image/bmp",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "wasm" => "application/wasm",
        "pdf" => "application/pdf",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" | "oga" => "audio/ogg",
        "m4a" => "audio/mp4",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "glb" => "model/gltf-binary",
        "gltf" => "model/gltf+json",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Request, ReadError> {
        read_request(&mut std::io::BufReader::new(text.as_bytes()), 16)
    }

    #[test]
    fn reads_a_request_with_its_query_and_body() {
        let request = parse(
            "PUT /_artifact/storage?key=a%2Fb%20c&x=1 HTTP/1.1\r\nHost: 127.0.0.1:23100\r\nContent-Length: 4\r\n\r\ntrueGET",
        )
        .unwrap();
        assert_eq!(request.method, "PUT");
        assert_eq!(request.path, "/_artifact/storage");
        assert_eq!(request.query("key").as_deref(), Some("a/b c"));
        assert_eq!(request.query("missing"), None);
        assert_eq!(request.header("host"), Some("127.0.0.1:23100"));
        assert_eq!(request.body, b"true");
        assert!(request.keeps_alive);

        let request = parse("GET /a%20b/%E2%9C%93.html HTTP/1.0\r\n\r\n").unwrap();
        assert_eq!(request.path, "/a b/✓.html");
        assert!(!request.keeps_alive);
    }

    #[test]
    fn refuses_what_it_does_not_read() {
        assert_eq!(parse("").unwrap_err(), ReadError::Closed);
        assert!(matches!(parse("hello\r\n\r\n"), Err(ReadError::Bad(_))));
        assert!(matches!(
            parse("GET http://example.com/ HTTP/1.1\r\n\r\n"),
            Err(ReadError::Bad(_))
        ));
        assert_eq!(
            parse("PUT /x HTTP/1.1\r\nContent-Length: 17\r\n\r\n").unwrap_err(),
            ReadError::TooLarge
        );
        let long = format!("GET / HTTP/1.1\r\nX: {}\r\n\r\n", "a".repeat(MAX_HEAD));
        assert!(matches!(parse(&long), Err(ReadError::Bad(_))));
        assert!(matches!(
            parse("GET /%FF HTTP/1.1\r\n\r\n"),
            Err(ReadError::Bad(_))
        ));
    }

    #[test]
    fn ranges_are_one_plain_span_or_the_whole_file() {
        assert_eq!(range("bytes=0-99", 1000), Some((0, 99)));
        assert_eq!(range("bytes=900-", 1000), Some((900, 999)));
        assert_eq!(range("bytes=-100", 1000), Some((900, 999)));
        assert_eq!(range("bytes=0-5000", 1000), Some((0, 999)));
        assert_eq!(range("bytes=1000-", 1000), None);
        assert_eq!(range("bytes=0-1,5-6", 1000), None);
        assert_eq!(range("lines=1-2", 1000), None);
    }

    #[test]
    fn an_answer_says_its_length_and_never_carries_a_second_line() {
        let mut out = Vec::new();
        Response::text(200, "hi")
            .header("ETag", "\"a\"\r\nSet-Cookie: x")
            .write(&mut out, false, true)
            .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n"));
        assert!(text.contains("ETag: \"a\"  Set-Cookie: x\r\n"));
        assert!(text.ends_with("\r\n\r\nhi"));
        assert_eq!(
            content_type(Path::new("a/B.HTML")),
            "text/html; charset=utf-8"
        );
        assert_eq!(content_type(Path::new("x")), "application/octet-stream");
    }
}
