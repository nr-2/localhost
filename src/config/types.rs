use std::collections::HashMap;

/// HTTP methods the server understands at the configuration level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Method {
    Get,
    Post,
    Delete,
}

impl Method {
    pub fn from_str(s: &str) -> Option<Method> {
        match s {
            "GET" => Some(Method::Get),
            "POST" => Some(Method::Post),
            "DELETE" => Some(Method::Delete),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Delete => "DELETE",
        }
    }
}

/// A `location` block inside a `server` block.
#[derive(Debug, Clone)]
pub struct Route {
    /// The path prefix this route matches, e.g. "/" or "/uploads".
    pub path: String,
    /// HTTP methods allowed on this route.
    pub methods: Vec<Method>,
    /// Filesystem directory this route is rooted to.
    pub root: Option<String>,
    /// Default file served when the request targets a directory.
    pub index: Option<String>,
    /// Whether directory listing is enabled when no index is present.
    pub autoindex: bool,
    /// HTTP redirection: (status code, target location).
    pub redirect: Option<(u16, String)>,
    /// Map of file extension (including the leading dot) to CGI interpreter path.
    pub cgi: HashMap<String, String>,
    /// Directory where uploaded files are stored. Defaults to `root` when not set.
    pub upload_store: Option<String>,
}

impl Route {
    pub fn new(path: String) -> Self {
        Route {
            path,
            methods: vec![Method::Get],
            root: None,
            index: None,
            autoindex: false,
            redirect: None,
            cgi: HashMap::new(),
            upload_store: None,
        }
    }
}

/// A single `server { ... }` block.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Address to bind to, e.g. "0.0.0.0" or "127.0.0.1".
    pub host: String,
    /// All ports this server should listen on.
    pub ports: Vec<u16>,
    /// `server_name` values used to disambiguate virtual hosts.
    pub server_names: Vec<String>,
    /// Maximum accepted size, in bytes, for a request body.
    pub client_max_body_size: usize,
    /// Map of status code to a custom error page path (relative to cwd or absolute).
    pub error_pages: HashMap<u16, String>,
    /// Ordered list of routes/locations.
    pub routes: Vec<Route>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            host: "0.0.0.0".to_string(),
            ports: Vec::new(),
            server_names: Vec::new(),
            client_max_body_size: 1024 * 1024,
            error_pages: HashMap::new(),
            routes: Vec::new(),
        }
    }
}

/// The whole configuration file: a list of virtual servers.
#[derive(Debug, Clone)]
pub struct Config {
    pub servers: Vec<ServerConfig>,
}

impl ServerConfig {
    /// Finds the best matching route for `path` using longest-prefix matching.
    pub fn match_route(&self, path: &str) -> Option<&Route> {
        let mut best: Option<&Route> = None;
        for route in &self.routes {
            if path == route.path || path.starts_with(route.path.as_str()) {
                // Make sure the prefix match happens on a path-segment boundary,
                // unless the route path is exactly "/".
                let boundary_ok = route.path == "/"
                    || path.len() == route.path.len()
                    || route.path.ends_with('/')
                    || path.as_bytes().get(route.path.len()) == Some(&b'/');
                if !boundary_ok {
                    continue;
                }
                match best {
                    None => best = Some(route),
                    Some(b) if route.path.len() > b.path.len() => best = Some(route),
                    _ => {}
                }
            }
        }
        best
    }
}
