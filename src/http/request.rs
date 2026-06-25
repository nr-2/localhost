/// Errors that can occur while incrementally parsing an HTTP request.
/// Each variant maps to the HTTP status code that should be returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    BadRequest,
    HeaderTooLarge,
    UriTooLong,
    PayloadTooLarge,
    NotImplemented,
    VersionNotSupported,
}

impl ParseError {
    pub fn status_code(&self) -> u16 {
        match self {
            ParseError::BadRequest => 400,
            ParseError::HeaderTooLarge => 431,
            ParseError::UriTooLong => 414,
            ParseError::PayloadTooLarge => 413,
            ParseError::NotImplemented => 501,
            ParseError::VersionNotSupported => 505,
        }
    }
}

#[derive(Debug, Clone)]
enum State {
    RequestLine,
    Headers,
    BodyContentLength(usize),
    BodyChunkedSize,
    BodyChunkedData(usize),
    BodyChunkedCRLF,
    BodyChunkedTrailer,
    Done,
}

/// A fully parsed HTTP request.
#[derive(Debug, Clone, Default)]
pub struct Request {
    pub method: String,
    pub target: String,
    pub path: String,
    pub query: String,
    pub version: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    /// Case-insensitive header lookup.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// Returns true if the connection should be kept alive after this request.
    pub fn keep_alive(&self) -> bool {
        match self.header("connection") {
            Some(v) => !v.to_lowercase().contains("close"),
            None => self.version == "HTTP/1.1",
        }
    }

    /// Returns the value of the `Cookie` header for a given cookie name, if present.
    pub fn cookie(&self, name: &str) -> Option<String> {
        let header = self.header("cookie")?;
        for part in header.split(';') {
            let part = part.trim();
            if let Some((k, v)) = part.split_once('=') {
                if k.trim() == name {
                    return Some(v.trim().to_string());
                }
            }
        }
        None
    }
}

/// Incremental, non-blocking-friendly HTTP/1.1 request parser.
///
/// Bytes are fed in as they arrive from a non-blocking socket via [`feed`].
/// The parser consumes only as many bytes as it can use, leaving the rest
/// in the buffer for the next call.
#[derive(Debug, Clone)]
pub struct RequestParser {
    state: State,
    method: String,
    target: String,
    version: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    header_bytes: usize,
}

impl RequestParser {
    pub fn new() -> Self {
        RequestParser {
            state: State::RequestLine,
            method: String::new(),
            target: String::new(),
            version: String::new(),
            headers: Vec::new(),
            body: Vec::new(),
            header_bytes: 0,
        }
    }

    /// Feeds bytes from `buf` into the parser, consuming what it can.
    /// Returns `Ok(true)` once the request (including body) is fully parsed.
    pub fn feed(
        &mut self,
        buf: &mut Vec<u8>,
        max_header: usize,
        max_body: usize,
    ) -> Result<bool, ParseError> {
        loop {
            match self.state.clone() {
                State::RequestLine => match find_line_end(buf) {
                    Some(pos) => {
                        let line = take_line(buf, pos);
                        if line.len() > max_header {
                            return Err(ParseError::UriTooLong);
                        }
                        self.parse_request_line(&line)?;
                        self.state = State::Headers;
                    }
                    None => {
                        if buf.len() > max_header {
                            return Err(ParseError::UriTooLong);
                        }
                        return Ok(false);
                    }
                },
                State::Headers => match find_line_end(buf) {
                    Some(pos) => {
                        let line = take_line(buf, pos);
                        if line.is_empty() {
                            self.state = self.compute_body_state(max_body)?;
                        } else {
                            self.header_bytes += line.len() + 2;
                            if self.header_bytes > max_header {
                                return Err(ParseError::HeaderTooLarge);
                            }
                            self.parse_header_line(&line)?;
                        }
                    }
                    None => {
                        if self.header_bytes + buf.len() > max_header {
                            return Err(ParseError::HeaderTooLarge);
                        }
                        return Ok(false);
                    }
                },
                State::BodyContentLength(remaining) => {
                    if remaining == 0 {
                        self.state = State::Done;
                        continue;
                    }
                    if buf.is_empty() {
                        return Ok(false);
                    }
                    let take = remaining.min(buf.len());
                    self.body.extend_from_slice(&buf[..take]);
                    buf.drain(..take);
                    let left = remaining - take;
                    self.state = State::BodyContentLength(left);
                    if left == 0 {
                        self.state = State::Done;
                    } else {
                        return Ok(false);
                    }
                }
                State::BodyChunkedSize => match find_line_end(buf) {
                    Some(pos) => {
                        let line = take_line(buf, pos);
                        let size_str = line.split(';').next().unwrap_or("").trim();
                        if size_str.is_empty() {
                            return Err(ParseError::BadRequest);
                        }
                        let size = usize::from_str_radix(size_str, 16)
                            .map_err(|_| ParseError::BadRequest)?;
                        if size == 0 {
                            self.state = State::BodyChunkedTrailer;
                        } else {
                            if self.body.len() + size > max_body {
                                return Err(ParseError::PayloadTooLarge);
                            }
                            self.state = State::BodyChunkedData(size);
                        }
                    }
                    None => {
                        if buf.len() > 64 {
                            // A chunk-size line should never be this long.
                            return Err(ParseError::BadRequest);
                        }
                        return Ok(false);
                    }
                },
                State::BodyChunkedData(remaining) => {
                    if remaining == 0 {
                        self.state = State::BodyChunkedCRLF;
                        continue;
                    }
                    if buf.is_empty() {
                        return Ok(false);
                    }
                    let take = remaining.min(buf.len());
                    self.body.extend_from_slice(&buf[..take]);
                    buf.drain(..take);
                    let left = remaining - take;
                    self.state = State::BodyChunkedData(left);
                    if left != 0 {
                        return Ok(false);
                    }
                }
                State::BodyChunkedCRLF => {
                    if buf.len() < 2 {
                        return Ok(false);
                    }
                    if buf.starts_with(b"\r\n") {
                        buf.drain(..2);
                    } else {
                        return Err(ParseError::BadRequest);
                    }
                    self.state = State::BodyChunkedSize;
                }
                State::BodyChunkedTrailer => match find_line_end(buf) {
                    Some(pos) => {
                        let line = take_line(buf, pos);
                        if line.is_empty() {
                            self.state = State::Done;
                        }
                        // Trailer header lines (if any) are ignored.
                    }
                    None => return Ok(false),
                },
                State::Done => return Ok(true),
            }
        }
    }

    fn parse_request_line(&mut self, line: &str) -> Result<(), ParseError> {
        let parts: Vec<&str> = line.split(' ').filter(|p| !p.is_empty()).collect();
        if parts.len() != 3 {
            return Err(ParseError::BadRequest);
        }
        self.method = parts[0].to_string();
        self.target = parts[1].to_string();
        self.version = parts[2].to_string();

        if !self.target.starts_with('/') {
            return Err(ParseError::BadRequest);
        }
        if self.version != "HTTP/1.0" && self.version != "HTTP/1.1" {
            return Err(ParseError::VersionNotSupported);
        }
        if self.method.is_empty() || !self.method.chars().all(|c| c.is_ascii_uppercase()) {
            return Err(ParseError::BadRequest);
        }
        Ok(())
    }

    fn parse_header_line(&mut self, line: &str) -> Result<(), ParseError> {
        let (name, value) = line.split_once(':').ok_or(ParseError::BadRequest)?;
        let name = name.trim();
        let value = value.trim();
        if name.is_empty() || name.contains(' ') {
            return Err(ParseError::BadRequest);
        }
        self.headers.push((name.to_string(), value.to_string()));
        Ok(())
    }

    fn header_value(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn compute_body_state(&mut self, max_body: usize) -> Result<State, ParseError> {
        if let Some(te) = self.header_value("transfer-encoding") {
            let te = te.to_lowercase();
            if te.contains("chunked") {
                return Ok(State::BodyChunkedSize);
            }
            return Err(ParseError::NotImplemented);
        }
        if let Some(cl) = self.header_value("content-length") {
            let len: usize = cl.trim().parse().map_err(|_| ParseError::BadRequest)?;
            if len > max_body {
                return Err(ParseError::PayloadTooLarge);
            }
            if len == 0 {
                return Ok(State::Done);
            }
            return Ok(State::BodyContentLength(len));
        }
        Ok(State::Done)
    }

    /// Consumes the parser, producing a [`Request`]. Only meaningful once
    /// [`feed`] has returned `Ok(true)`.
    pub fn into_request(self) -> Request {
        let (path, query) = match self.target.split_once('?') {
            Some((p, q)) => (p.to_string(), q.to_string()),
            None => (self.target.clone(), String::new()),
        };
        let path = percent_decode(&path);
        Request {
            method: self.method,
            target: self.target,
            path,
            query,
            version: self.version,
            headers: self.headers,
            body: self.body,
        }
    }
}

/// Finds the offset of `\n` in `buf`, used to delimit request-line/header lines.
fn find_line_end(buf: &[u8]) -> Option<usize> {
    buf.iter().position(|&b| b == b'\n')
}

/// Removes the line (up to and including the `\n` at `nl_pos`) from `buf` and
/// returns it as a `String` with the trailing `\r` (if any) stripped.
fn take_line(buf: &mut Vec<u8>, nl_pos: usize) -> String {
    let mut end = nl_pos;
    if end > 0 && buf[end - 1] == b'\r' {
        end -= 1;
    }
    let line = String::from_utf8_lossy(&buf[..end]).into_owned();
    buf.drain(..=nl_pos);
    line
}

/// Decodes `%XX` percent-escapes and `+` is left as-is (path component, not query).
pub fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&input[i + 1..i + 3], 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_get() {
        let mut p = RequestParser::new();
        let mut buf = b"GET /index.html HTTP/1.1\r\nHost: localhost\r\n\r\n".to_vec();
        let done = p.feed(&mut buf, 8192, 1024).unwrap();
        assert!(done);
        let req = p.into_request();
        assert_eq!(req.method, "GET");
        assert_eq!(req.path, "/index.html");
        assert_eq!(req.header("host"), Some("localhost"));
    }

    #[test]
    fn parses_partial_then_complete() {
        let mut p = RequestParser::new();
        let mut buf = b"GET / HTTP/1.1\r\nHost: loc".to_vec();
        assert_eq!(p.feed(&mut buf, 8192, 1024).unwrap(), false);
        buf.extend_from_slice(b"alhost\r\n\r\n");
        assert_eq!(p.feed(&mut buf, 8192, 1024).unwrap(), true);
    }

    #[test]
    fn parses_content_length_body() {
        let mut p = RequestParser::new();
        let mut buf = b"POST /x HTTP/1.1\r\nContent-Length: 5\r\n\r\nhello".to_vec();
        assert!(p.feed(&mut buf, 8192, 1024).unwrap());
        let req = p.into_request();
        assert_eq!(req.body, b"hello");
    }

    #[test]
    fn parses_chunked_body() {
        let mut p = RequestParser::new();
        let mut buf = b"POST /x HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nWiki\r\n5\r\npedia\r\n0\r\n\r\n".to_vec();
        assert!(p.feed(&mut buf, 8192, 1024).unwrap());
        let req = p.into_request();
        assert_eq!(req.body, b"Wikipedia");
    }

    #[test]
    fn rejects_payload_too_large() {
        let mut p = RequestParser::new();
        let mut buf = b"POST /x HTTP/1.1\r\nContent-Length: 100\r\n\r\n".to_vec();
        let err = p.feed(&mut buf, 8192, 10).unwrap_err();
        assert_eq!(err, ParseError::PayloadTooLarge);
    }

    #[test]
    fn decodes_percent_encoding() {
        assert_eq!(percent_decode("/a%20b"), "/a b");
        assert_eq!(percent_decode("/100%25"), "/100%");
    }
}
