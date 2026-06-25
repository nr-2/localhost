pub mod cgi;
pub mod connection;
pub mod epoll;
pub mod io_util;
pub mod socket;

use std::collections::HashMap;
use std::io;
use std::mem;
use std::os::unix::io::RawFd;
use std::panic::{self, AssertUnwindSafe};
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::handler::{self, errors, session::SessionStore, HandlerResult};
use crate::http::{Request, Response};
use crate::util::find_subslice;

use cgi::CgiProcess;
use connection::Connection;
use epoll::{Epoll, EPOLLERR, EPOLLHUP, EPOLLIN, EPOLLOUT, EPOLLRDHUP};
use io_util::{close_fd, read_fd, would_block, write_fd};
use socket::{accept_conn, create_listener};

const MAX_HEADER_SIZE: usize = 16 * 1024;
const READ_CHUNK: usize = 64 * 1024;
const MAX_EVENTS: usize = 1024;
const EPOLL_WAIT_MS: i32 = 1000;
const CONN_TIMEOUT: Duration = Duration::from_secs(60);
const CGI_TIMEOUT: Duration = Duration::from_secs(15);
const SESSION_MAX_AGE: Duration = Duration::from_secs(30 * 60);

/// Identifies what a registered file descriptor represents.
#[derive(Debug, Clone, Copy)]
enum FdKind {
    Listener(usize),
    Client,
    CgiStdout(RawFd),
    CgiStdin(RawFd),
}

struct ListenerInfo {
    fd: RawFd,
    port: u16,
    server_indices: Vec<usize>,
}


pub struct Server {
    config: Config,
    epoll: Epoll,
    listeners: Vec<ListenerInfo>,
    fd_kind: HashMap<RawFd, FdKind>,
    connections: HashMap<RawFd, Connection>,
    sessions: SessionStore,
}

impl Server {
    pub fn new(config: Config) -> io::Result<Self> {
        let epoll = Epoll::new()?;
        let mut listeners = Vec::new();
        let mut fd_kind = HashMap::new();

        let mut listener_map: HashMap<(String, u16), Vec<usize>> = HashMap::new();
        for (idx, server) in config.servers.iter().enumerate() {
            for &port in &server.ports {
                listener_map
                    .entry((server.host.clone(), port))
                    .or_default()
                    .push(idx);
            }
        }

        let mut bound_any = false;
        for ((host, port), server_indices) in listener_map {
            match create_listener(&host, port) {
                Ok(fd) => {
                    let listener_idx = listeners.len();
                    epoll.add(fd, EPOLLIN)?;
                    fd_kind.insert(fd, FdKind::Listener(listener_idx));
                    listeners.push(ListenerInfo {
                        fd,
                        port,
                        server_indices,
                    });
                    println!("listening on {}:{}", host, port);
                    bound_any = true;
                }
                Err(e) => {
                    eprintln!("warning: could not bind {}:{}: {}", host, port, e);
                }
            }
        }

        if !bound_any {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "no listening sockets could be created",
            ));
        }

        Ok(Server {
            config,
            epoll,
            listeners,
            fd_kind,
            connections: HashMap::new(),
            sessions: SessionStore::new(),
        })
    }

    /// Runs the single-threaded event loop. Never returns under normal
    /// operation.
    pub fn run(&mut self) -> io::Result<()> {
        let mut events: Vec<libc::epoll_event> = vec![unsafe { mem::zeroed() }; MAX_EVENTS];
        let mut last_session_cleanup = Instant::now();

        loop {
            let n = self.epoll.wait(&mut events, EPOLL_WAIT_MS)?;
            for ev in events.iter().take(n) {
                let fd = ev.u64 as RawFd;
                let flags = ev.events;
                let kind = match self.fd_kind.get(&fd) {
                    Some(k) => *k,
                    None => continue, // stale event for an fd we already removed
                };

                let result = panic::catch_unwind(AssertUnwindSafe(|| {
                    self.dispatch_event(fd, flags, kind);
                }));
                if result.is_err() {
                    eprintln!("warning: recovered from panic while handling fd {}", fd);
                    self.close_connection_quiet(fd);
                }
            }

            let result = panic::catch_unwind(AssertUnwindSafe(|| {
                self.check_timeouts();
            }));
            if result.is_err() {
                eprintln!("warning: recovered from panic during timeout sweep");
            }

            if last_session_cleanup.elapsed() > Duration::from_secs(60) {
                self.sessions.cleanup(SESSION_MAX_AGE);
                last_session_cleanup = Instant::now();
            }

            reap_zombies();
        }
    }

    fn dispatch_event(&mut self, fd: RawFd, flags: u32, kind: FdKind) {
        match kind {
            FdKind::Listener(listener_idx) => self.accept_new_connections(listener_idx),
            FdKind::Client => self.handle_client_event(fd, flags),
            FdKind::CgiStdout(client_fd) => self.handle_cgi_stdout(client_fd, flags),
            FdKind::CgiStdin(client_fd) => self.handle_cgi_stdin(client_fd, flags),
        }
    }

    // Accepting connections
    fn accept_new_connections(&mut self, listener_idx: usize) {
        let (listen_fd, port, server_indices) = {
            let l = &self.listeners[listener_idx];
            (l.fd, l.port, l.server_indices.clone())
        };

        loop {
            match accept_conn(listen_fd) {
                Ok(Some((fd, peer))) => {
                    if let Err(e) = self.epoll.add(fd, EPOLLIN | EPOLLRDHUP) {
                        eprintln!("warning: epoll_ctl(ADD) failed for new connection: {}", e);
                        close_fd(fd);
                        continue;
                    }
                    self.fd_kind.insert(fd, FdKind::Client);
                    self.connections
                        .insert(fd, Connection::new(peer, port, server_indices.clone()));
                }
                Ok(None) => break,
                Err(e) => {
                    eprintln!("warning: accept() failed: {}", e);
                    break;
                }
            }
        }
    }

    // Client socket events

    fn handle_client_event(&mut self, fd: RawFd, flags: u32) {
        if flags & (EPOLLERR | EPOLLHUP) != 0 && flags & EPOLLIN == 0 {
            self.close_connection(fd);
            return;
        }
        if flags & EPOLLIN != 0 {
            self.handle_client_readable(fd);
            if !self.connections.contains_key(&fd) {
                return;
            }
        }
        if flags & EPOLLOUT != 0 {
            self.handle_client_writable(fd);
        }
    }

    fn handle_client_readable(&mut self, fd: RawFd) {
        // While a CGI is running for this connection we ignore further
        // reads until the response has been produced.
        if self
            .connections
            .get(&fd)
            .map(|c| c.cgi.is_some())
            .unwrap_or(false)
        {
            return;
        }

        let mut buf = [0u8; READ_CHUNK];
        let mut peer_closed = false;

        match read_fd(fd, &mut buf) {
            Ok(0) => {
                peer_closed = true;
            }
            Ok(n) => {
                if let Some(conn) = self.connections.get_mut(&fd) {
                    conn.read_buf.extend_from_slice(&buf[..n]);
                    conn.has_partial_request = true;
                    conn.last_active = Instant::now();
                }
            }
            Err(e) if would_block(&e) => {}
            Err(_) => {
                peer_closed = true;
            }
        }

        self.try_parse_and_process(fd);

        if peer_closed {
            let awaiting_write = self
                .connections
                .get(&fd)
                .map(|c| c.awaiting_write)
                .unwrap_or(false);
            if awaiting_write {
                if let Some(conn) = self.connections.get_mut(&fd) {
                    conn.should_close_after_write = true;
                }
            } else {
                self.close_connection(fd);
            }
        }
    }

    fn handle_client_writable(&mut self, fd: RawFd) {
        let mut done = false;
        let mut broken = false;

        if let Some(conn) = self.connections.get_mut(&fd) {
            if conn.write_pos >= conn.write_buf.len() {
                done = true;
            } else {
                match write_fd(fd, conn.write_remaining()) {
                    Ok(0) => {
                        broken = true;
                    }
                    Ok(n) => {
                        conn.write_pos += n;
                        conn.last_active = Instant::now();
                        done = conn.write_pos >= conn.write_buf.len();
                    }
                    Err(e) if would_block(&e) => {}
                    Err(_) => {
                        broken = true;
                    }
                }
            }
        }

        if broken {
            self.close_connection(fd);
            return;
        }

        if !done {
            return;
        }

        let should_close = self
            .connections
            .get(&fd)
            .map(|c| c.should_close_after_write)
            .unwrap_or(true);
        if should_close {
            self.close_connection(fd);
            return;
        }

        if let Some(conn) = self.connections.get_mut(&fd) {
            conn.reset_for_next_request();
        }
        let _ = self.epoll.modify(fd, EPOLLIN | EPOLLRDHUP);

        // Handle any pipelined request already sitting in read_buf.
        let has_leftover = self
            .connections
            .get(&fd)
            .map(|c| !c.read_buf.is_empty())
            .unwrap_or(false);
        if has_leftover {
            self.try_parse_and_process(fd);
        }
    }

    /// Feeds buffered bytes to the request parser and, once a full request
    /// is available, routes and dispatches it.
    fn try_parse_and_process(&mut self, fd: RawFd) {
        let mut completed = false;
        let mut parse_err = None;

        if let Some(conn) = self.connections.get_mut(&fd) {
            let max_body = self.config.servers[conn.server_indices[0]].client_max_body_size;
            let mut taken = mem::take(&mut conn.read_buf);
            match conn.parser.feed(&mut taken, MAX_HEADER_SIZE, max_body) {
                Ok(true) => completed = true,
                Ok(false) => {}
                Err(e) => parse_err = Some(e),
            }
            conn.read_buf = taken;
        }

        if let Some(err) = parse_err {
            self.respond_status_and_close(fd, err.status_code());
        } else if completed {
            self.process_request(fd);
        }
    }

    fn process_request(&mut self, fd: RawFd) {
        let (req, local_port, server_indices, peer_addr) = {
            let conn = match self.connections.get_mut(&fd) {
                Some(c) => c,
                None => return,
            };
            let parser = mem::replace(&mut conn.parser, crate::http::RequestParser::new());
            (
                parser.into_request(),
                conn.local_port,
                conn.server_indices.clone(),
                conn.peer_addr.clone(),
            )
        };

        let keep_alive = req.keep_alive();
        let server_idx = select_server(&self.config, &server_indices, &req);

        let result = {
            let server = &self.config.servers[server_idx];
            handler::handle_request(&req, server, local_port, &peer_addr, &mut self.sessions)
        };

        match result {
            HandlerResult::Response(mut resp) => {
                let bytes = resp.serialize(keep_alive);
                if let Some(conn) = self.connections.get_mut(&fd) {
                    conn.queue_response(bytes, !keep_alive);
                }
                let _ = self.epoll.modify(fd, EPOLLOUT | EPOLLRDHUP);
            }
            HandlerResult::Cgi(cgi_req) => {
                self.start_cgi(fd, cgi_req, server_idx, keep_alive);
            }
        }
    }

    // ------------------------------------------------------------------
    // CGI
    // ------------------------------------------------------------------

    fn start_cgi(
        &mut self,
        fd: RawFd,
        cgi_req: handler::CgiRequest,
        server_idx: usize,
        keep_alive: bool,
    ) {
        match CgiProcess::spawn(
            &cgi_req.interpreter,
            &cgi_req.script_path,
            &cgi_req.cwd,
            &cgi_req.env,
            cgi_req.body,
        ) {
            Ok(cgi) => {
                if let Some(out_fd) = cgi.stdout_fd {
                    if self.epoll.add(out_fd, EPOLLIN).is_err() {
                        self.fail_cgi(fd, server_idx, keep_alive);
                        return;
                    }
                    self.fd_kind.insert(out_fd, FdKind::CgiStdout(fd));
                }
                if let Some(in_fd) = cgi.stdin_fd {
                    if self.epoll.add(in_fd, EPOLLOUT).is_err() {
                        self.fail_cgi(fd, server_idx, keep_alive);
                        return;
                    }
                    self.fd_kind.insert(in_fd, FdKind::CgiStdin(fd));
                }

                // Stop reading further request data while the CGI runs.
                let _ = self.epoll.modify(fd, EPOLLRDHUP);

                if let Some(conn) = self.connections.get_mut(&fd) {
                    conn.cgi_keep_alive = keep_alive;
                    conn.cgi_server_idx = server_idx;
                    conn.cgi_session = Some((cgi_req.session_id, cgi_req.new_session));
                    conn.cgi = Some(cgi);
                    conn.last_active = Instant::now();
                }
            }
            Err(_) => self.fail_cgi(fd, server_idx, keep_alive),
        }
    }

    fn fail_cgi(&mut self, fd: RawFd, server_idx: usize, keep_alive: bool) {
        let mut resp = errors::error_response(502, &self.config.servers[server_idx]);
        let bytes = resp.serialize(keep_alive);
        if let Some(conn) = self.connections.get_mut(&fd) {
            conn.queue_response(bytes, !keep_alive);
        }
        let _ = self.epoll.modify(fd, EPOLLOUT | EPOLLRDHUP);
    }

    fn handle_cgi_stdout(&mut self, client_fd: RawFd, flags: u32) {
        let mut buf = [0u8; READ_CHUNK];
        let mut eof = flags & (EPOLLHUP | EPOLLERR) != 0;

        let cgi_fd = match self
            .connections
            .get(&client_fd)
            .and_then(|c| c.cgi.as_ref())
            .and_then(|c| c.stdout_fd)
        {
            Some(f) => f,
            None => return,
        };
        match read_fd(cgi_fd, &mut buf) {
            Ok(0) => {
                eof = true;
            }
            Ok(n) => {
                if let Some(conn) = self.connections.get_mut(&client_fd) {
                    if let Some(cgi) = conn.cgi.as_mut() {
                        cgi.output.extend_from_slice(&buf[..n]);
                    }
                    conn.last_active = Instant::now();
                }
            }
            Err(e) if would_block(&e) => {}
            Err(_) => {
                eof = true;
            }
        }

        if eof {
            self.finish_cgi(client_fd);
        }
    }

    fn handle_cgi_stdin(&mut self, client_fd: RawFd, flags: u32) {
        let mut finished = flags & (EPOLLHUP | EPOLLERR) != 0;
        let mut in_fd_opt = None;

        if let Some(conn) = self.connections.get_mut(&client_fd) {
            if let Some(cgi) = conn.cgi.as_mut() {
                if let Some(in_fd) = cgi.stdin_fd {
                    in_fd_opt = Some(in_fd);
                    if !finished {
                        if cgi.input_pos >= cgi.input.len() {
                            finished = true;
                        } else {
                            match write_fd(in_fd, &cgi.input[cgi.input_pos..]) {
                                Ok(0) => {
                                    finished = true;
                                }
                                Ok(n) => {
                                    cgi.input_pos += n;
                                    finished = cgi.input_pos >= cgi.input.len();
                                }
                                Err(e) if would_block(&e) => {}
                                Err(_) => {
                                    finished = true;
                                }
                            }
                        }
                    }
                    conn.last_active = Instant::now();
                }
            }
        }

        if finished {
            if let Some(in_fd) = in_fd_opt {
                self.epoll.remove(in_fd);
                close_fd(in_fd);
                self.fd_kind.remove(&in_fd);
                if let Some(conn) = self.connections.get_mut(&client_fd) {
                    if let Some(cgi) = conn.cgi.as_mut() {
                        cgi.stdin_fd = None;
                    }
                }
            }
        }
    }

    /// Called once the CGI's stdout has reached EOF: reaps the process,
    /// builds the HTTP response from its output, and queues it for writing.
    fn finish_cgi(&mut self, client_fd: RawFd) {
        let (output, keep_alive, server_idx, session_info, stdin_fd, stdout_fd) = {
            let conn = match self.connections.get_mut(&client_fd) {
                Some(c) => c,
                None => return,
            };
            let cgi = match conn.cgi.as_mut() {
                Some(c) => c,
                None => return,
            };
            cgi.try_reap();
            (
                mem::take(&mut cgi.output),
                conn.cgi_keep_alive,
                conn.cgi_server_idx,
                conn.cgi_session.take(),
                cgi.stdin_fd.take(),
                cgi.stdout_fd.take(),
            )
        };

        if let Some(f) = stdin_fd {
            self.epoll.remove(f);
            close_fd(f);
            self.fd_kind.remove(&f);
        }
        if let Some(f) = stdout_fd {
            self.epoll.remove(f);
            close_fd(f);
            self.fd_kind.remove(&f);
        }

        let server = &self.config.servers[server_idx];
        let mut resp = build_cgi_response(&output, server);
        if let Some((sid, is_new)) = session_info {
            if is_new {
                resp.headers.push((
                    "Set-Cookie".to_string(),
                    format!("session_id={}; Path=/; HttpOnly", sid),
                ));
            }
        }
        let bytes = resp.serialize(keep_alive);

        if let Some(conn) = self.connections.get_mut(&client_fd) {
            conn.cgi = None;
            conn.queue_response(bytes, !keep_alive);
        }
        let _ = self.epoll.modify(client_fd, EPOLLOUT | EPOLLRDHUP);
    }

    // ------------------------------------------------------------------
    // Timeouts & teardown
    // ------------------------------------------------------------------

    fn check_timeouts(&mut self) {
        let now = Instant::now();
        let mut respond_408 = Vec::new();
        let mut respond_504 = Vec::new();
        let mut to_close = Vec::new();

        for (&fd, conn) in self.connections.iter() {
            let elapsed = now.duration_since(conn.last_active);
            if conn.cgi.is_some() {
                if elapsed > CGI_TIMEOUT {
                    respond_504.push(fd);
                }
            } else if elapsed > CONN_TIMEOUT {
                if conn.awaiting_write {
                    to_close.push(fd);
                } else if conn.has_partial_request {
                    respond_408.push(fd);
                } else {
                    to_close.push(fd);
                }
            }
        }

        for fd in respond_504 {
            self.kill_cgi(fd);
            self.respond_status_and_close(fd, 504);
        }
        for fd in respond_408 {
            self.respond_status_and_close(fd, 408);
        }
        for fd in to_close {
            self.close_connection(fd);
        }
    }

    fn kill_cgi(&mut self, fd: RawFd) {
        if let Some(conn) = self.connections.get_mut(&fd) {
            if let Some(mut cgi) = conn.cgi.take() {
                if let Some(f) = cgi.stdin_fd.take() {
                    self.epoll.remove(f);
                    close_fd(f);
                    self.fd_kind.remove(&f);
                }
                if let Some(f) = cgi.stdout_fd.take() {
                    self.epoll.remove(f);
                    close_fd(f);
                    self.fd_kind.remove(&f);
                }
                cgi.kill();
            }
        }
    }

    fn respond_status_and_close(&mut self, fd: RawFd, code: u16) {
        let server_idx = match self.connections.get(&fd) {
            Some(c) => c.server_indices[0],
            None => return,
        };
        let mut resp = errors::error_response(code, &self.config.servers[server_idx]);
        let bytes = resp.serialize(false);
        if let Some(conn) = self.connections.get_mut(&fd) {
            conn.queue_response(bytes, true);
        }
        let _ = self.epoll.modify(fd, EPOLLOUT | EPOLLRDHUP);
    }

    fn close_connection(&mut self, fd: RawFd) {
        self.kill_cgi(fd);
        self.connections.remove(&fd);
        self.epoll.remove(fd);
        close_fd(fd);
        self.fd_kind.remove(&fd);
    }

    /// Like [`close_connection`] but used from the panic-recovery path,
    /// where `fd` might belong to a CGI pipe rather than a client socket.
    fn close_connection_quiet(&mut self, fd: RawFd) {
        if self.connections.contains_key(&fd) {
            self.close_connection(fd);
            return;
        }
        if let Some(kind) = self.fd_kind.get(&fd).copied() {
            match kind {
                FdKind::CgiStdout(client_fd) | FdKind::CgiStdin(client_fd) => {
                    self.close_connection(client_fd);
                }
                _ => {
                    self.epoll.remove(fd);
                    close_fd(fd);
                    self.fd_kind.remove(&fd);
                }
            }
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        for &fd in self.connections.keys().collect::<Vec<_>>() {
            close_fd(fd);
        }
        for l in &self.listeners {
            close_fd(l.fd);
        }
    }
}

/// Picks the virtual host that should handle `req` among `candidates`
/// (indices into `Config::servers` sharing a `(host, port)`), based on the
/// `Host` header. Falls back to the first candidate (the default server).
fn select_server(config: &Config, candidates: &[usize], req: &Request) -> usize {
    if candidates.len() == 1 {
        return candidates[0];
    }
    if let Some(host_header) = req.header("host") {
        let host_name = host_header.split(':').next().unwrap_or(host_header);
        for &idx in candidates {
            if config.servers[idx]
                .server_names
                .iter()
                .any(|n| n == host_name)
            {
                return idx;
            }
        }
    }
    candidates[0]
}

/// Parses a CGI script's output (CGI-style headers, blank line, body) into
/// an HTTP [`Response`]. A `Status:` header overrides the default 200.
fn build_cgi_response(output: &[u8], server: &crate::config::ServerConfig) -> Response {
    let split = find_subslice(output, b"\r\n\r\n")
        .map(|p| (p, 4))
        .or_else(|| find_subslice(output, b"\n\n").map(|p| (p, 2)));

    match split {
        Some((pos, sep_len)) => {
            let header_part = String::from_utf8_lossy(&output[..pos]).into_owned();
            let body = &output[pos + sep_len..];

            let mut status = 200u16;
            let mut resp = Response::new(200);
            for line in header_part.split('\n') {
                let line = line.trim_end_matches('\r');
                if line.is_empty() {
                    continue;
                }
                if let Some((k, v)) = line.split_once(':') {
                    let k = k.trim();
                    let v = v.trim();
                    if k.eq_ignore_ascii_case("status") {
                        if let Some(code_str) = v.split_whitespace().next() {
                            status = code_str.parse().unwrap_or(200);
                        }
                    } else {
                        resp.set_header(k, v);
                    }
                }
            }
            resp.status = status;
            resp.body = body.to_vec();
            resp
        }
        None => {
            if output.is_empty() {
                errors::error_response(502, server)
            } else {
                Response::new(200)
                    .with_header("Content-Type", "text/plain; charset=utf-8")
                    .with_body(output.to_vec())
            }
        }
    }
}

/// Reaps any exited CGI children that are still zombies. `CgiProcess::kill`
/// and `try_reap` only handle the child each `CgiProcess` was spawned for,
/// and a `SIGKILL`'d child may not be immediately waitable; calling this on
/// every loop iteration guarantees no zombies accumulate regardless of
/// timing.
fn reap_zombies() {
    loop {
        let mut status: i32 = 0;
        let pid = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
        if pid <= 0 {
            break;
        }
    }
}
