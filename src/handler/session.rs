use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::time::{Duration, Instant};

/// Server-side data associated with a single browser session.
pub struct Session {
    pub data: HashMap<String, String>,
    pub last_access: Instant,
}

/// A simple in-memory session store keyed by an opaque session id that is
/// handed to clients via a `Set-Cookie: session_id=...` header.
pub struct SessionStore {
    sessions: HashMap<String, Session>,
}

impl SessionStore {
    pub fn new() -> Self {
        SessionStore {
            sessions: HashMap::new(),
        }
    }

    /// Looks up `existing` (the session id sent by the client, if any).
    /// Returns the session id to use (existing or freshly generated) and
    /// whether it is new (so the caller can send `Set-Cookie`).
    pub fn resolve(&mut self, existing: Option<&str>) -> (String, bool) {
        if let Some(id) = existing {
            if self.sessions.contains_key(id) {
                self.sessions.get_mut(id).unwrap().last_access = Instant::now();
                return (id.to_string(), false);
            }
        }
        let id = generate_id();
        self.sessions.insert(
            id.clone(),
            Session {
                data: HashMap::new(),
                last_access: Instant::now(),
            },
        );
        (id, true)
    }

    /// Increments and returns the visit counter for a session.
    pub fn record_visit(&mut self, id: &str) -> u32 {
        let session = self
            .sessions
            .entry(id.to_string())
            .or_insert_with(|| Session {
                data: HashMap::new(),
                last_access: Instant::now(),
            });
        session.last_access = Instant::now();
        let counter = session
            .data
            .entry("visits".to_string())
            .or_insert_with(|| "0".to_string());
        let next = counter.parse::<u32>().unwrap_or(0) + 1;
        *counter = next.to_string();
        next
    }

    /// Drops sessions that have not been accessed for longer than `max_age`.
    pub fn cleanup(&mut self, max_age: Duration) {
        let now = Instant::now();
        self.sessions
            .retain(|_, session| now.duration_since(session.last_access) < max_age);
    }
}

/// Generates a 32-character hex session id from `/dev/urandom`, falling back
/// to a time-based value if that is unavailable.
fn generate_id() -> String {
    let mut buf = [0u8; 16];
    let mut filled = false;
    if let Ok(mut f) = File::open("/dev/urandom") {
        if f.read_exact(&mut buf).is_ok() {
            filled = true;
        }
    }
    if !filled {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        for (i, byte) in buf.iter_mut().enumerate() {
            *byte = ((nanos >> (i * 8)) & 0xff) as u8;
        }
    }
    buf.iter().map(|b| format!("{:02x}", b)).collect()
}
