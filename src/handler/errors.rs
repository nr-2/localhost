use std::fs;

use crate::config::ServerConfig;
use crate::http::{reason_phrase, Response};

/// Builds an error response for `code`, preferring a custom error page
/// configured via `error_page` if one exists and can be read, falling back
/// to a built-in default page otherwise.
pub fn error_response(code: u16, server: &ServerConfig) -> Response {
    if let Some(path) = server.error_pages.get(&code) {
        if let Ok(content) = fs::read(path) {
            return Response::html(code, content);
        }
    }
    Response::html(code, default_error_page(code))
}

/// A minimal, self-contained HTML page describing the error.
pub fn default_error_page(code: u16) -> Vec<u8> {
    let reason = reason_phrase(code);
    let description = match code {
        400 => "The server could not understand the request due to invalid syntax.",
        403 => "You do not have permission to access this resource.",
        404 => "The requested resource could not be found on this server.",
        405 => "The HTTP method used is not allowed for this resource.",
        408 => "The server timed out waiting for the request.",
        411 => "The request did not specify the length of its content.",
        413 => "The request body is larger than the server is willing to process.",
        414 => "The request-target is longer than the server is willing to interpret.",
        431 => "The request's header fields were too large.",
        500 => "The server encountered an unexpected error and could not complete the request.",
        501 => "The server does not support the functionality required to fulfill the request.",
        502 => "The server received an invalid response from an upstream resource (CGI).",
        503 => "The server is temporarily unable to handle the request.",
        504 => "An upstream resource (CGI) took too long to respond.",
        505 => "The HTTP version used in the request is not supported.",
        _ => "An error occurred while processing the request.",
    };

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>{code} {reason}</title>
<style>
  body {{ font-family: -apple-system, Segoe UI, Helvetica, Arial, sans-serif; background:#0f172a; color:#e2e8f0; display:flex; align-items:center; justify-content:center; height:100vh; margin:0; }}
  .card {{ text-align:center; padding:2.5rem 3rem; border-radius:12px; background:#1e293b; box-shadow:0 10px 30px rgba(0,0,0,.4); }}
  h1 {{ font-size:4rem; margin:0; color:#38bdf8; }}
  h2 {{ margin:.25rem 0 1rem; font-weight:500; color:#f1f5f9; }}
  p {{ color:#94a3b8; max-width:32ch; margin:0 auto; }}
  a {{ color:#38bdf8; text-decoration:none; }}
</style>
</head>
<body>
  <div class="card">
    <h1>{code}</h1>
    <h2>{reason}</h2>
    <p>{description}</p>
    <p style="margin-top:1.5rem;"><a href="/">Return home</a></p>
  </div>
</body>
</html>
"#,
        code = code,
        reason = reason,
        description = description
    )
    .into_bytes()
}
