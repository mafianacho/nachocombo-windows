//! A deliberately small HTTP/1.1 reader and writer (port of `ComputerUseHTTP.swift`): one request per connection,
//! `Content-Length` bodies only, hard size caps. Chunked bodies are refused (411); `Expect: 100-continue` is ignored
//! (the client sends the body after its own short wait).
use serde_json::json;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: String,
    pub path: String,
    /// Header names are lower-cased.
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

impl HttpRequest {
    pub fn header(&self, name: &str) -> Option<&str> { self.headers.get(&name.to_ascii_lowercase()).map(String::as_str) }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn new(status: u16) -> Self { Self { status, headers: vec![], body: vec![] } }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    pub fn status_text(status: u16) -> &'static str {
        match status {
            200 => "OK",
            202 => "Accepted",
            400 => "Bad Request",
            401 => "Unauthorized",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            411 => "Length Required",
            413 => "Payload Too Large",
            429 => "Too Many Requests",
            431 => "Request Header Fields Too Large",
            503 => "Service Unavailable",
            _ => "Error",
        }
    }

    /// One response per connection (`Connection: close`): no keep-alive state to leak or to time out.
    pub fn serialized(&self) -> Vec<u8> {
        let mut text = format!("HTTP/1.1 {} {}\r\n", self.status, Self::status_text(self.status));
        for (name, value) in &self.headers {
            if name.eq_ignore_ascii_case("content-length") || name.eq_ignore_ascii_case("connection") { continue; }
            text.push_str(&format!("{name}: {value}\r\n"));
        }
        text.push_str(&format!("Content-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n", self.body.len()));
        let mut out = text.into_bytes();
        out.extend_from_slice(&self.body);
        out
    }

    pub fn json(status: u16, body: Vec<u8>, extra: Vec<(String, String)>) -> Self {
        let mut headers = vec![("Content-Type".to_string(), "application/json".to_string())];
        headers.extend(extra);
        Self { status, headers, body }
    }

    pub fn error(status: u16, message: &str) -> Self { Self::error_with(status, message, vec![]) }

    pub fn error_with(status: u16, message: &str, extra: Vec<(String, String)>) -> Self {
        Self::json(status, serde_json::to_vec(&json!({"error": message})).unwrap_or_default(), extra)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HttpParse {
    NeedMore,
    Request(HttpRequest, usize),
    Invalid(HttpResponse),
}

pub struct HttpParser;

impl HttpParser {
    pub const MAX_HEADER_BYTES: usize = 16 * 1024;
    pub const MAX_BODY_BYTES: usize = 1024 * 1024;

    pub fn parse(buffer: &[u8]) -> HttpParse { Self::parse_with(buffer, Self::MAX_HEADER_BYTES, Self::MAX_BODY_BYTES) }

    pub fn parse_with(buffer: &[u8], max_header: usize, max_body: usize) -> HttpParse {
        let Some(head_end) = buffer.windows(4).position(|w| w == b"\r\n\r\n") else {
            return if buffer.len() > max_header { HttpParse::Invalid(HttpResponse::error(431, "headers too large")) } else { HttpParse::NeedMore };
        };
        if head_end > max_header { return HttpParse::Invalid(HttpResponse::error(431, "headers too large")); }
        let bad = || HttpParse::Invalid(HttpResponse::error(400, "bad request"));
        let Ok(head) = std::str::from_utf8(&buffer[..head_end]) else { return bad() };
        let mut lines = head.split("\r\n");
        let request_line: Vec<&str> = lines.next().unwrap_or("").split(' ').filter(|p| !p.is_empty()).collect();
        if request_line.len() != 3 || !request_line[2].starts_with("HTTP/1.") || !request_line[1].starts_with('/')
            || request_line[0].is_empty() || !request_line[0].chars().all(|c| c.is_ascii_alphabetic()) {
            return bad();
        }
        let mut headers = BTreeMap::new();
        for line in lines.filter(|l| !l.is_empty()) {
            let Some(colon) = line.find(':') else { return bad() };
            if colon == 0 { return bad(); }
            let name = line[..colon].trim().to_ascii_lowercase();
            let value = line[colon + 1..].trim().to_string();
            if name == "content-length" && headers.get(&name).is_some_and(|v: &String| *v != value) { return bad(); }
            headers.insert(name, value);
        }
        if headers.contains_key("transfer-encoding") { return HttpParse::Invalid(HttpResponse::error(411, "content-length required")); }
        let mut length = 0usize;
        if let Some(raw) = headers.get("content-length") {
            if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) { return bad(); }
            let Ok(n) = raw.parse::<usize>() else { return bad() };
            if n > max_body { return HttpParse::Invalid(HttpResponse::error(413, "body too large")); }
            length = n;
        }
        let body_start = head_end + 4;
        if buffer.len() < body_start + length { return HttpParse::NeedMore; }
        let request = HttpRequest {
            method: request_line[0].to_ascii_uppercase(),
            path: request_line[1].to_string(),
            headers,
            body: buffer[body_start..body_start + length].to_vec(),
        };
        HttpParse::Request(request, body_start + length)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(raw: &str) -> HttpParse { HttpParser::parse(raw.as_bytes()) }

    #[test]
    fn parses_a_post_with_a_body() {
        let raw = "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 2\r\nAuthorization: Bearer x\r\n\r\n{}";
        let HttpParse::Request(r, used) = req(&format!("{raw}extra")) else { panic!() };
        assert_eq!(r.method, "POST");
        assert_eq!(r.path, "/mcp");
        assert_eq!(r.header("AUTHORIZATION"), Some("Bearer x"));
        assert_eq!(r.body, b"{}");
        assert_eq!(used, raw.len());
    }

    #[test]
    fn waits_for_the_rest() {
        assert_eq!(req("POST /mcp HTTP/1.1\r\nContent-Length: 5\r\n\r\n{}"), HttpParse::NeedMore);
        assert_eq!(req("POST /mcp HTTP/1.1\r\nHost: x"), HttpParse::NeedMore);
    }

    #[test]
    fn refuses_what_it_does_not_support() {
        let status = |p: HttpParse| match p { HttpParse::Invalid(r) => r.status, other => panic!("{other:?}") };
        assert_eq!(status(req("POST /mcp HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n")), 411);
        assert_eq!(status(req("POST /mcp HTTP/1.1\r\nContent-Length: 2000000\r\n\r\n")), 413);
        assert_eq!(status(req("POST /mcp HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\n")), 400);
        assert_eq!(status(req("POST /mcp HTTP/1.1\r\nContent-Length: -1\r\n\r\n")), 400);
        assert_eq!(status(req("POST mcp HTTP/1.1\r\n\r\n")), 400);
        assert_eq!(status(req("POST /mcp SPDY/3\r\n\r\n")), 400);
        assert_eq!(status(req("POST /mcp HTTP/1.1\r\nno colon\r\n\r\n")), 400);
        let big = format!("POST /mcp HTTP/1.1\r\nX: {}\r\n", "a".repeat(17 * 1024));
        assert_eq!(status(HttpParser::parse(big.as_bytes())), 431);
    }

    #[test]
    fn writes_one_response_and_closes() {
        let r = HttpResponse::error_with(401, "unauthorized", vec![("WWW-Authenticate".into(), "Bearer".into()), ("Connection".into(), "keep-alive".into())]);
        let text = String::from_utf8(r.serialized()).unwrap();
        assert!(text.starts_with("HTTP/1.1 401 Unauthorized\r\n"));
        assert!(text.contains("WWW-Authenticate: Bearer\r\n"));
        assert!(text.contains("Connection: close\r\n"));
        assert!(!text.contains("keep-alive"));
        assert!(text.ends_with("\r\n\r\n{\"error\":\"unauthorized\"}"));
        assert!(text.contains("Content-Length: 24\r\n"));
    }
}
