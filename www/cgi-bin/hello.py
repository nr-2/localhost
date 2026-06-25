#!/usr/bin/env python3
"""A minimal CGI/1.1 script: reads the request body from stdin (terminated
by EOF, as required by CGI), inspects its environment, and writes a
CGI response (headers, blank line, body) to stdout."""

import os
import sys
from datetime import datetime, timezone


def read_body():
    try:
        length = int(os.environ.get("CONTENT_LENGTH", "0") or "0")
    except ValueError:
        length = 0
    if length <= 0:
        return b""
    return sys.stdin.buffer.read(length)


def html_escape(s):
    return (
        s.replace("&", "&amp;")
        .replace("<", "&lt;")
        .replace(">", "&gt;")
    )


def main():
    body = read_body()
    method = os.environ.get("REQUEST_METHOD", "GET")
    query = os.environ.get("QUERY_STRING", "")
    path_info = os.environ.get("PATH_INFO", "")
    remote = os.environ.get("REMOTE_ADDR", "")
    now = datetime.now(timezone.utc).strftime("%Y-%m-%d %H:%M:%S UTC")

    rows = "".join(
        "<tr><td><code>{}</code></td><td><code>{}</code></td></tr>\n".format(
            html_escape(k), html_escape(v)
        )
        for k, v in sorted(os.environ.items())
        if k.startswith(("REQUEST_", "SCRIPT_", "SERVER_", "QUERY_", "PATH_", "CONTENT_", "HTTP_"))
    )

    body_html = html_escape(body.decode("utf-8", "replace")) if body else "(empty)"

    html = """<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>hello.py - CGI</title>
<link rel="stylesheet" href="/style.css">
</head>
<body>
<main>
  <h1>Hello from Python CGI</h1>
  <div class="card">
    <p>Time: {now}</p>
    <p>Method: <code>{method}</code> &mdash; Path info: <code>{path_info}</code> &mdash; Query: <code>{query}</code></p>
    <p>Remote address: <code>{remote}</code></p>
  </div>
  <h2>Request body</h2>
  <div class="card"><pre>{body}</pre></div>
  <h2>CGI environment</h2>
  <div class="card">
    <table>{rows}</table>
  </div>
  <p><a href="/">&larr; Back home</a></p>
</main>
</body>
</html>
""".format(now=now, method=method, path_info=path_info, query=query, remote=remote, body=body_html, rows=rows)

    out = html.encode("utf-8")
    sys.stdout.write("Status: 200 OK\r\n")
    sys.stdout.write("Content-Type: text/html; charset=utf-8\r\n")
    sys.stdout.write("Content-Length: {}\r\n".format(len(out)))
    sys.stdout.write("\r\n")
    sys.stdout.flush()
    sys.stdout.buffer.write(out)


if __name__ == "__main__":
    main()
