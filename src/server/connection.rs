use std::time::Instant;

use super::cgi::CgiProcess;
use crate::http::RequestParser;


pub struct Connection {
    pub peer_addr: String,
    pub local_port: u16,
   
    pub server_indices: Vec<usize>,

    pub read_buf: Vec<u8>,
    pub write_buf: Vec<u8>,
    pub write_pos: usize,
    pub parser: RequestParser,

    pub last_active: Instant,
    pub awaiting_write: bool,
    pub should_close_after_write: bool,

    pub has_partial_request: bool,

    /// CGI process handling the current request, if any.
    pub cgi: Option<CgiProcess>,
    pub cgi_keep_alive: bool,
    pub cgi_server_idx: usize,

    pub cgi_session: Option<(String, bool)>,
}

impl Connection {
    pub fn new(peer_addr: String, local_port: u16, server_indices: Vec<usize>) -> Self {
        Connection {
            peer_addr,
            local_port,
            server_indices,
            read_buf: Vec::new(),
            write_buf: Vec::new(),
            write_pos: 0,
            parser: RequestParser::new(),
            last_active: Instant::now(),
            awaiting_write: false,
            should_close_after_write: false,
            has_partial_request: false,
            cgi: None,
            cgi_keep_alive: false,
            cgi_server_idx: 0,
            cgi_session: None,
        }
    }

    /// Prepares the connection to parse a new request after the previous
    pub fn reset_for_next_request(&mut self) {
        self.parser = RequestParser::new();
        self.write_buf.clear();
        self.write_pos = 0;
        self.awaiting_write = false;
        self.has_partial_request = false;
        self.cgi = None;
        self.cgi_session = None;
    }

    /// Queues `bytes` to be written out, replacing any previous response.
    pub fn queue_response(&mut self, bytes: Vec<u8>, close_after: bool) {
        self.write_buf = bytes;
        self.write_pos = 0;
        self.awaiting_write = true;
        self.should_close_after_write = close_after;
    }

    pub fn write_remaining(&self) -> &[u8] {
        &self.write_buf[self.write_pos..]
    }
}
