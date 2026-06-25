/// Returns the standard reason phrase for an HTTP status code.
pub fn reason_phrase(code: u16) -> &'static str {
    match code {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        304 => "Not Modified",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        411 => "Length Required",
        413 => "Payload Too Large",
        414 => "URI Too Long",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        505 => "HTTP Version Not Supported",
        _ => "Unknown",
    }
}

/// An HTTP/1.1 response being built up before being serialized to the wire.
#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn new(status: u16) -> Self {
        Response {
            status,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    pub fn with_header(mut self, name: &str, value: impl Into<String>) -> Self {
        self.set_header(name, value);
        self
    }

    pub fn set_header(&mut self, name: &str, value: impl Into<String>) {
        let value = value.into();
        if let Some(entry) = self
            .headers
            .iter_mut()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
        {
            entry.1 = value;
        } else {
            self.headers.push((name.to_string(), value));
        }
    }

    pub fn has_header(&self, name: &str) -> bool {
        self.headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case(name))
    }

    pub fn with_body(mut self, body: Vec<u8>) -> Self {
        self.body = body;
        self
    }

    pub fn html(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Response::new(status)
            .with_header("Content-Type", "text/html; charset=utf-8")
            .with_body(body.into())
    }

    pub fn text(status: u16, body: impl Into<String>) -> Self {
        Response::new(status)
            .with_header("Content-Type", "text/plain; charset=utf-8")
            .with_body(body.into().into_bytes())
    }

    /// Serializes the response into raw HTTP/1.1 bytes ready to be written to a socket.
    /// `keep_alive` controls the `Connection` header.
    pub fn serialize(&mut self, keep_alive: bool) -> Vec<u8> {
        if !self.has_header("Content-Length") {
            let len = self.body.len();
            self.set_header("Content-Length", len.to_string());
        }
        if !self.has_header("Connection") {
            self.set_header(
                "Connection",
                if keep_alive { "keep-alive" } else { "close" },
            );
        }
        if !self.has_header("Server") {
            self.set_header("Server", "localhost/0.1");
        }

        let mut out = Vec::with_capacity(self.body.len() + 256);
        out.extend_from_slice(
            format!(
                "HTTP/1.1 {} {}\r\n",
                self.status,
                reason_phrase(self.status)
            )
            .as_bytes(),
        );
        for (k, v) in &self.headers {
            out.extend_from_slice(format!("{}: {}\r\n", k, v).as_bytes());
        }
        out.extend_from_slice(b"\r\n");
        out.extend_from_slice(&self.body);
        out
    }
}
