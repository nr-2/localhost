use std::collections::HashMap;

/// HTTP methods the server 
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
    pub path: String,
    pub methods: Vec<Method>,
    pub root: Option<String>,
    pub index: Option<String>,
    pub autoindex: bool,
    pub redirect: Option<(u16, String)>,
    pub cgi: HashMap<String, String>,
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
    pub host: String,
    pub ports: Vec<u16>,
    pub server_names: Vec<String>,
    pub client_max_body_size: usize,
    pub error_pages: HashMap<u16, String>,
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
    ///  using longest-prefix matching.
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
