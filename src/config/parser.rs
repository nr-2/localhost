use std::collections::{HashMap, HashSet};
use std::fs;

use super::types::{Config, Method, Route, ServerConfig};

/// Parses a configuration file at `path` into a [`Config`].
pub fn parse_file(path: &str) -> Result<Config, String> {
    let content = fs::read_to_string(path)
        .map_err(|e| format!("cannot read configuration file '{}': {}", path, e))?;
    parse_str(&content)
}

/// Parses configuration source text into a [`Config`].
pub fn parse_str(content: &str) -> Result<Config, String> {
    let lines = clean_lines(content);
    let mut i = 0;
    let mut servers = Vec::new();

    while i < lines.len() {
        let tokens: Vec<&str> = lines[i].split_whitespace().collect();
        if tokens.len() == 2 && tokens[0] == "server" && tokens[1] == "{" {
            i += 1;
            servers.push(parse_server_block(&lines, &mut i)?);
        } else {
            return Err(format!(
                "unexpected directive '{}' at top level (expected 'server {{')",
                lines[i]
            ));
        }
    }

    if servers.is_empty() {
        return Err("configuration must declare at least one 'server' block".to_string());
    }

    for server in servers.iter_mut() {
        if server.ports.is_empty() {
            server.ports.push(8080);
        }
        if server.routes.is_empty() {
            let mut root = Route::new("/".to_string());
            root.root = Some("./www/html".to_string());
            root.index = Some("index.html".to_string());
            server.routes.push(root);
        }
    }

    validate_listener_conflicts(&servers)?;

    Ok(Config { servers })
}

/// Strips `#` comments and blank lines, trimming surrounding whitespace.
fn clean_lines(content: &str) -> Vec<String> {
    content
        .lines()
        .map(|line| match line.find('#') {
            Some(idx) => &line[..idx],
            None => line,
        })
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect()
}

fn parse_server_block(lines: &[String], i: &mut usize) -> Result<ServerConfig, String> {
    let mut server = ServerConfig::default();
    let mut listen_seen: HashSet<(String, u16)> = HashSet::new();

    while *i < lines.len() {
        let line = lines[*i].clone();
        if line == "}" {
            *i += 1;
            return Ok(server);
        }

        let tokens: Vec<&str> = line.split_whitespace().collect();
        match tokens[0] {
            "location" => {
                if tokens.len() < 3 || tokens[tokens.len() - 1] != "{" {
                    return Err(format!("invalid 'location' directive: '{}'", line));
                }
                let path = tokens[1].to_string();
                *i += 1;
                server.routes.push(parse_location_block(lines, i, path)?);
            }
            "listen" => {
                require_args(&tokens, 1, &line)?;
                let value = tokens[1];
                let (host, port) = if let Some(idx) = value.rfind(':') {
                    let host = value[..idx].to_string();
                    let port: u16 = value[idx + 1..]
                        .parse()
                        .map_err(|_| format!("invalid port in 'listen {}'", value))?;
                    (host, port)
                } else {
                    let port: u16 = value
                        .parse()
                        .map_err(|_| format!("invalid port in 'listen {}'", value))?;
                    (server.host.clone(), port)
                };
                if !listen_seen.insert((host.clone(), port)) {
                    return Err(format!("duplicate listen directive '{}:{}'", host, port));
                }
                server.host = host;
                server.ports.push(port);
                *i += 1;
            }
            "host" | "server_address" => {
                require_args(&tokens, 1, &line)?;
                server.host = tokens[1].to_string();
                *i += 1;
            }
            "server_name" => {
                require_args(&tokens, 1, &line)?;
                server.server_names = tokens[1..].iter().map(|s| s.to_string()).collect();
                *i += 1;
            }
            "client_max_body_size" => {
                require_args(&tokens, 1, &line)?;
                server.client_max_body_size = parse_size(tokens[1])?;
                *i += 1;
            }
            "error_page" => {
                if tokens.len() < 3 {
                    return Err(format!("invalid 'error_page' directive: '{}'", line));
                }
                let path = tokens[tokens.len() - 1].to_string();
                for code_str in &tokens[1..tokens.len() - 1] {
                    let code: u16 = code_str
                        .parse()
                        .map_err(|_| format!("invalid status code '{}' in error_page", code_str))?;
                    server.error_pages.insert(code, path.clone());
                }
                *i += 1;
            }
            other => return Err(format!("unknown directive '{}' in server block", other)),
        }
    }

    Err("unexpected end of file: missing '}' to close 'server' block".to_string())
}

fn parse_location_block(lines: &[String], i: &mut usize, path: String) -> Result<Route, String> {
    let mut route = Route::new(path);
    let mut methods_explicit = false;

    while *i < lines.len() {
        let line = lines[*i].clone();
        if line == "}" {
            *i += 1;
            if !methods_explicit && route.redirect.is_none() {
                // Default to GET when nothing is specified.
                route.methods = vec![Method::Get];
            }
            return Ok(route);
        }

        let tokens: Vec<&str> = line.split_whitespace().collect();
        match tokens[0] {
            "root" => {
                require_args(&tokens, 1, &line)?;
                route.root = Some(tokens[1].to_string());
                *i += 1;
            }
            "index" => {
                require_args(&tokens, 1, &line)?;
                route.index = Some(tokens[1].to_string());
                *i += 1;
            }
            "autoindex" => {
                require_args(&tokens, 1, &line)?;
                route.autoindex = tokens[1] == "on" || tokens[1] == "true";
                *i += 1;
            }
            "methods" => {
                if tokens.len() < 2 {
                    return Err(format!(
                        "'methods' requires at least one method: '{}'",
                        line
                    ));
                }
                let mut methods = Vec::new();
                for m in &tokens[1..] {
                    match Method::from_str(m) {
                        Some(meth) => methods.push(meth),
                        None => return Err(format!("unsupported HTTP method '{}'", m)),
                    }
                }
                route.methods = methods;
                methods_explicit = true;
                *i += 1;
            }
            "return" | "redirect" => {
                if tokens.len() < 3 {
                    return Err(format!("invalid '{}' directive: '{}'", tokens[0], line));
                }
                let code: u16 = tokens[1]
                    .parse()
                    .map_err(|_| format!("invalid redirect status code '{}'", tokens[1]))?;
                route.redirect = Some((code, tokens[2].to_string()));
                *i += 1;
            }
            "cgi" => {
                if tokens.len() < 3 {
                    return Err(format!("invalid 'cgi' directive: '{}'", line));
                }
                route
                    .cgi
                    .insert(tokens[1].to_string(), tokens[2].to_string());
                *i += 1;
            }
            "upload_store" => {
                require_args(&tokens, 1, &line)?;
                route.upload_store = Some(tokens[1].to_string());
                *i += 1;
            }
            other => return Err(format!("unknown directive '{}' in location block", other)),
        }
    }

    Err("unexpected end of file: missing '}' to close 'location' block".to_string())
}

fn require_args(tokens: &[&str], min_extra: usize, line: &str) -> Result<(), String> {
    if tokens.len() < 1 + min_extra {
        return Err(format!("missing argument(s) for directive: '{}'", line));
    }
    Ok(())
}

/// Parses sizes such as `10`, `10K`, `10M`, `10G` (case-insensitive, base 1024).
fn parse_size(value: &str) -> Result<usize, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("empty size value".to_string());
    }
    let (num_part, mult) = match value.chars().last().unwrap() {
        'k' | 'K' => (&value[..value.len() - 1], 1024usize),
        'm' | 'M' => (&value[..value.len() - 1], 1024 * 1024),
        'g' | 'G' => (&value[..value.len() - 1], 1024 * 1024 * 1024),
        _ => (value, 1usize),
    };
    let num: usize = num_part
        .trim()
        .parse()
        .map_err(|_| format!("invalid size value '{}'", value))?;
    Ok(num * mult)
}

fn validate_listener_conflicts(servers: &[ServerConfig]) -> Result<(), String> {
    let mut listener_map: HashMap<(String, u16), Vec<usize>> = HashMap::new();

    for (idx, server) in servers.iter().enumerate() {
        for &port in &server.ports {
            listener_map
                .entry((server.host.clone(), port))
                .or_default()
                .push(idx);
        }
    }

    for ((host, port), indices) in listener_map {
        if indices.len() < 2 {
            continue;
        }

        let mut seen_names = HashSet::new();
        let mut unnamed_block = None;

        for idx in indices {
            let server = &servers[idx];
            if server.server_names.is_empty() {
                unnamed_block = Some(idx);
                break;
            }
            for name in &server.server_names {
                if !seen_names.insert(name.clone()) {
                    return Err(format!(
                        "duplicate listen endpoint '{}:{}' shared by multiple server blocks using server_name '{}'",
                        host, port, name
                    ));
                }
            }
        }

        if let Some(idx) = unnamed_block {
            return Err(format!(
                "duplicate listen endpoint '{}:{}' shared by server block {} without a unique server_name",
                host,
                port,
                idx + 1
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_config() {
        let cfg = parse_str(
            r#"
            server {
                listen 8080
                location / {
                    root ./www/html
                    index index.html
                    methods GET POST DELETE
                    autoindex on
                }
            }
            "#,
        )
        .unwrap();
        assert_eq!(cfg.servers.len(), 1);
        let s = &cfg.servers[0];
        assert_eq!(s.ports, vec![8080]);
        assert_eq!(s.routes.len(), 1);
        assert_eq!(s.routes[0].root, Some("./www/html".to_string()));
        assert!(s.routes[0].autoindex);
    }

    #[test]
    fn parses_multiple_servers_and_redirects() {
        let cfg = parse_str(
            r#"
            server {
                listen 127.0.0.1:8080
                server_name example.com
                client_max_body_size 10M
                error_page 404 /errors/404.html

                location /old {
                    return 301 /new
                }
            }
            server {
                listen 8081
                location / {
                    root ./www
                }
            }
            "#,
        )
        .unwrap();
        assert_eq!(cfg.servers.len(), 2);
        assert_eq!(cfg.servers[0].host, "127.0.0.1");
        assert_eq!(cfg.servers[0].client_max_body_size, 10 * 1024 * 1024);
        assert_eq!(
            cfg.servers[0].routes[0].redirect,
            Some((301, "/new".to_string()))
        );
    }

    #[test]
    fn rejects_unknown_directive() {
        let err = parse_str(
            r#"
            server {
                bogus value
            }
            "#,
        )
        .unwrap_err();
        assert!(err.contains("unknown directive"));
    }

    #[test]
    fn rejects_duplicate_listen_in_same_server() {
        let err = parse_str(
            r#"
            server {
                listen 127.0.0.1:8080
                listen 127.0.0.1:8080
            }
            "#,
        )
        .unwrap_err();
        assert!(err.contains("duplicate listen directive"));
    }

    #[test]
    fn allows_shared_listener_for_distinct_server_names() {
        let cfg = parse_str(
            r#"
            server {
                listen 127.0.0.1:8080
                server_name one.test
            }
            server {
                listen 127.0.0.1:8080
                server_name two.test
            }
            "#,
        )
        .unwrap();
        assert_eq!(cfg.servers.len(), 2);
    }

    #[test]
    fn rejects_shared_listener_without_distinct_names() {
        let err = parse_str(
            r#"
            server {
                listen 127.0.0.1:8080
            }
            server {
                listen 127.0.0.1:8080
            }
            "#,
        )
        .unwrap_err();
        assert!(err.contains("duplicate listen endpoint"));
    }
}
