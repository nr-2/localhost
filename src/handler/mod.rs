pub mod dirlisting;
pub mod errors;
pub mod mime;
pub mod path_resolve;
pub mod session;
pub mod upload;

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::{Method as CfgMethod, Route, ServerConfig};
use crate::http::{Request, Response};
use session::SessionStore;

/// Outcome of routing+dispatching a request.
pub enum HandlerResult {
    /// The response is fully ready to be sent.
    Response(Response),
    /// The request must be handed off to a CGI process.
    Cgi(CgiRequest),
}

/// Everything needed to fork and run a CGI script for a request.
pub struct CgiRequest {
    pub interpreter: String,
    pub script_path: String,
    pub cwd: String,
    pub env: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub session_id: String,
    pub new_session: bool,
}

/// Top-level entry point: routes `req` against `server` and returns either a
/// ready response or a CGI invocation. Handles session cookie assignment.
pub fn handle_request(
    req: &Request,
    server: &ServerConfig,
    server_port: u16,
    remote_addr: &str,
    sessions: &mut SessionStore,
) -> HandlerResult {
    let existing = req.cookie("session_id");
    let (session_id, is_new) = sessions.resolve(existing.as_deref());

    let mut result = dispatch(req, server, server_port, remote_addr, &session_id, sessions);

    match &mut result {
        HandlerResult::Response(resp) => {
            if is_new {
                resp.headers.push((
                    "Set-Cookie".to_string(),
                    format!("session_id={}; Path=/; HttpOnly", session_id),
                ));
            }
        }
        HandlerResult::Cgi(cgi) => {
            cgi.session_id = session_id;
            cgi.new_session = is_new;
        }
    }

    result
}

fn dispatch(
    req: &Request,
    server: &ServerConfig,
    port: u16,
    remote_addr: &str,
    session_id: &str,
    sessions: &mut SessionStore,
) -> HandlerResult {
    if req.path == "/session" {
        return HandlerResult::Response(session_page(session_id, sessions));
    }

    let route = match server.match_route(&req.path) {
        Some(r) => r,
        None => return HandlerResult::Response(errors::error_response(404, server)),
    };

    if let Some((code, target)) = &route.redirect {
        let resp = Response::new(*code).with_header("Location", target.clone());
        return HandlerResult::Response(resp);
    }

    let method = match req.method.as_str() {
        "GET" => Some(CfgMethod::Get),
        "POST" => Some(CfgMethod::Post),
        "DELETE" => Some(CfgMethod::Delete),
        _ => None,
    };
    let method = match method {
        Some(m) => m,
        None => return HandlerResult::Response(errors::error_response(501, server)),
    };
    if !route.methods.contains(&method) {
        let allow = route
            .methods
            .iter()
            .map(|m| m.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let mut resp = errors::error_response(405, server);
        resp.set_header("Allow", allow);
        return HandlerResult::Response(resp);
    }

    let fs_path = match path_resolve::resolve(route, &req.path) {
        Some(p) => p,
        None => return HandlerResult::Response(errors::error_response(403, server)),
    };

    match method {
        CfgMethod::Get => handle_get(req, server, route, &fs_path, port, remote_addr),
        CfgMethod::Post => handle_post(req, server, route, &fs_path, port, remote_addr),
        CfgMethod::Delete => handle_delete(server, &fs_path),
    }
}

fn handle_get(
    req: &Request,
    server: &ServerConfig,
    route: &Route,
    fs_path: &Path,
    port: u16,
    remote_addr: &str,
) -> HandlerResult {
    match fs::metadata(fs_path) {
        Ok(meta) if meta.is_dir() => {
            if !req.path.ends_with('/') {
                return HandlerResult::Response(
                    Response::new(301).with_header("Location", format!("{}/", req.path)),
                );
            }
            if let Some(index) = &route.index {
                let index_path = fs_path.join(index);
                if index_path.is_file() {
                    return serve_file_or_cgi(req, server, route, &index_path, port, remote_addr);
                }
            }
            if route.autoindex {
                if let Some(html) = dirlisting::render(&fs_path.to_string_lossy(), &req.path) {
                    return HandlerResult::Response(Response::html(200, html));
                }
            }
            HandlerResult::Response(errors::error_response(403, server))
        }
        Ok(meta) if meta.is_file() => {
            serve_file_or_cgi(req, server, route, fs_path, port, remote_addr)
        }
        _ => HandlerResult::Response(errors::error_response(404, server)),
    }
}

fn handle_post(
    req: &Request,
    server: &ServerConfig,
    route: &Route,
    fs_path: &Path,
    port: u16,
    remote_addr: &str,
) -> HandlerResult {
    if fs_path.is_file() {
        if let Some(interpreter) = cgi_interpreter_for(route, fs_path) {
            return build_cgi_request(req, fs_path, &interpreter, port, remote_addr);
        }
    }

    let content_type = req.header("content-type").unwrap_or("");
    if content_type.starts_with("multipart/form-data") {
        return handle_multipart_upload(req, server, route, content_type);
    }

    handle_raw_upload(req, server, fs_path)
}

fn handle_multipart_upload(
    req: &Request,
    server: &ServerConfig,
    route: &Route,
    content_type: &str,
) -> HandlerResult {
    let boundary = match upload::extract_boundary(content_type) {
        Some(b) => b,
        None => return HandlerResult::Response(errors::error_response(400, server)),
    };

    let files = upload::parse_multipart(&req.body, &boundary);
    if files.is_empty() {
        return HandlerResult::Response(Response::text(400, "No file part found in upload\n"));
    }

    let store_dir = match route.upload_store.clone().or_else(|| route.root.clone()) {
        Some(d) => d,
        None => return HandlerResult::Response(errors::error_response(500, server)),
    };
    if fs::create_dir_all(&store_dir).is_err() {
        return HandlerResult::Response(errors::error_response(500, server));
    }

    let mut saved = Vec::new();
    for file in files {
        let name = upload::sanitize_filename(&file.filename);
        let dest = Path::new(&store_dir).join(&name);
        if fs::write(&dest, &file.data).is_err() {
            return HandlerResult::Response(errors::error_response(500, server));
        }
        saved.push(name);
    }

    let body = format!("Uploaded {} file(s): {}\n", saved.len(), saved.join(", "));
    HandlerResult::Response(Response::text(201, body))
}

fn handle_raw_upload(req: &Request, server: &ServerConfig, fs_path: &Path) -> HandlerResult {
    if let Some(parent) = fs_path.parent() {
        if fs::create_dir_all(parent).is_err() {
            return HandlerResult::Response(errors::error_response(500, server));
        }
    }

    let target = if fs_path.is_dir() || req.path.ends_with('/') {
        fs_path.join(format!("upload-{}.bin", unique_suffix()))
    } else {
        fs_path.to_path_buf()
    };

    let created = !target.exists();
    match fs::write(&target, &req.body) {
        Ok(_) => {
            let status = if created { 201 } else { 200 };
            HandlerResult::Response(Response::text(
                status,
                format!("Saved {} bytes to {}\n", req.body.len(), target.display()),
            ))
        }
        Err(_) => HandlerResult::Response(errors::error_response(500, server)),
    }
}

fn handle_delete(server: &ServerConfig, fs_path: &Path) -> HandlerResult {
    match fs::metadata(fs_path) {
        Ok(meta) if meta.is_file() => match fs::remove_file(fs_path) {
            Ok(_) => HandlerResult::Response(Response::new(204)),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                HandlerResult::Response(errors::error_response(403, server))
            }
            Err(_) => HandlerResult::Response(errors::error_response(500, server)),
        },
        Ok(meta) if meta.is_dir() => HandlerResult::Response(errors::error_response(403, server)),
        _ => HandlerResult::Response(errors::error_response(404, server)),
    }
}

fn serve_file_or_cgi(
    req: &Request,
    server: &ServerConfig,
    route: &Route,
    fs_path: &Path,
    port: u16,
    remote_addr: &str,
) -> HandlerResult {
    if let Some(interpreter) = cgi_interpreter_for(route, fs_path) {
        return build_cgi_request(req, fs_path, &interpreter, port, remote_addr);
    }
    match fs::read(fs_path) {
        Ok(data) => {
            let content_type = mime::mime_type(&fs_path.to_string_lossy());
            HandlerResult::Response(
                Response::new(200)
                    .with_header("Content-Type", content_type)
                    .with_body(data),
            )
        }
        Err(_) => HandlerResult::Response(errors::error_response(404, server)),
    }
}

fn cgi_interpreter_for(route: &Route, fs_path: &Path) -> Option<String> {
    let ext = fs_path.extension()?.to_str()?;
    route.cgi.get(&format!(".{}", ext)).cloned()
}

fn build_cgi_request(
    req: &Request,
    fs_path: &Path,
    interpreter: &str,
    port: u16,
    remote_addr: &str,
) -> HandlerResult {
    let abs_path = fs::canonicalize(fs_path).unwrap_or_else(|_| fs_path.to_path_buf());
    let script_path = abs_path.to_string_lossy().to_string();
    let cwd = abs_path
        .parent()
        .map(|p| {
            if p.as_os_str().is_empty() {
                "/".to_string()
            } else {
                p.to_string_lossy().to_string()
            }
        })
        .unwrap_or_else(|| "/".to_string());
    let mut env = vec![
        ("GATEWAY_INTERFACE".to_string(), "CGI/1.1".to_string()),
        ("SERVER_PROTOCOL".to_string(), "HTTP/1.1".to_string()),
        ("SERVER_SOFTWARE".to_string(), "localhost/0.1".to_string()),
        (
            "SERVER_NAME".to_string(),
            req.header("host")
                .map(|h| h.split(':').next().unwrap_or("localhost").to_string())
                .unwrap_or_else(|| "localhost".to_string()),
        ),
        ("SERVER_PORT".to_string(), port.to_string()),
        ("REMOTE_ADDR".to_string(), remote_addr.to_string()),
        ("REQUEST_METHOD".to_string(), req.method.clone()),
        ("REQUEST_URI".to_string(), req.target.clone()),
        ("SCRIPT_NAME".to_string(), req.path.clone()),
        ("SCRIPT_FILENAME".to_string(), script_path.clone()),
        ("PATH_INFO".to_string(), req.path.clone()),
        ("QUERY_STRING".to_string(), req.query.clone()),
        ("REDIRECT_STATUS".to_string(), "200".to_string()),
    ];

    if let Some(ct) = req.header("content-type") {
        env.push(("CONTENT_TYPE".to_string(), ct.to_string()));
    }
    env.push(("CONTENT_LENGTH".to_string(), req.body.len().to_string()));

    for (k, v) in &req.headers {
        if k.eq_ignore_ascii_case("content-type") || k.eq_ignore_ascii_case("content-length") {
            continue;
        }
        let key = format!("HTTP_{}", k.to_uppercase().replace('-', "_"));
        env.push((key, v.clone()));
    }

    HandlerResult::Cgi(CgiRequest {
        interpreter: interpreter.to_string(),
        script_path,
        cwd,
        env,
        body: req.body.clone(),
        session_id: String::new(),
        new_session: false,
    })
}

fn session_page(session_id: &str, sessions: &mut SessionStore) -> Response {
    let visits = sessions.record_visit(session_id);
    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>Session info</title>
<style>
  body {{ font-family: -apple-system, Segoe UI, Helvetica, Arial, sans-serif; background:#0f172a; color:#e2e8f0; display:flex; align-items:center; justify-content:center; height:100vh; margin:0; }}
  .card {{ text-align:center; padding:2.5rem 3rem; border-radius:12px; background:#1e293b; box-shadow:0 10px 30px rgba(0,0,0,.4); }}
  code {{ color:#38bdf8; }}
  a {{ color:#38bdf8; text-decoration:none; }}
</style>
</head>
<body>
  <div class="card">
    <h1>Session info</h1>
    <p>Session ID: <code>{session_id}</code></p>
    <p>Visits recorded in this session: <strong>{visits}</strong></p>
    <p style="margin-top:1.5rem;"><a href="/">Return home</a></p>
  </div>
</body>
</html>
"#,
        session_id = session_id,
        visits = visits
    );
    Response::html(200, html.into_bytes())
}

fn unique_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}
