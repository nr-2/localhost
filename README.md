# localhost

`localhost` is a single-threaded, non-blocking HTTP/1.1 server written in Rust
for the 42 Localhost/Webserv project.

It is built directly on Linux system calls through the `libc` crate. The server
does not use `tokio`, `nix`, Hyper, Actix, or any crate that already implements
HTTP server behavior.

## Features

- One process and one thread for the main server.
- One central `epoll` instance handles listener sockets, client sockets, and CGI
  pipes.
- Non-blocking I/O for sockets and CGI pipes.
- One read or write operation per ready file descriptor event.
- HTTP/1.1 request/response handling.
- `GET`, `POST`, and `DELETE` methods.
- Keep-alive and `Connection: close`.
- `Content-Length` and `Transfer-Encoding: chunked` request bodies.
- Static file serving with MIME types.
- Directory index files.
- Directory listing with `autoindex`.
- HTTP redirects.
- File uploads with `multipart/form-data` and raw request bodies.
- Cookies and in-memory sessions with a `session_id` cookie.
- Custom error pages for `400`, `403`, `404`, `405`, `413`, and `500`.
- CGI execution by file extension.
- Python CGI and shell CGI examples.
- Per-connection and per-CGI timeouts.
- `SIGPIPE` ignored so broken client connections do not kill the server.
- Panic recovery around the event loop so a bad request does not bring down the
  process.

## Build And Run

This project is Linux/WSL oriented because it uses `epoll`.

```sh
cargo build --release
./target/release/localhost conf/default.conf
```

If no config path is provided, the server uses:

```sh
conf/default.conf
```

The default config starts:

- `http://127.0.0.1:8080` for the main website, uploads, sessions, redirects,
  and CGI.
- `http://127.0.0.1:8081` for a second website.

## Configuration

The config format uses nginx-like blocks.

```nginx
server {
    listen 127.0.0.1:8080
    server_name localhost

    client_max_body_size 5M

    error_page 400 ./www/errors/400.html
    error_page 403 ./www/errors/403.html
    error_page 404 ./www/errors/404.html
    error_page 405 ./www/errors/405.html
    error_page 413 ./www/errors/413.html
    error_page 500 ./www/errors/500.html

    location / {
        root ./www/html
        index index.html
        methods GET POST DELETE
        autoindex off
    }

    location /uploads {
        root ./www/uploads
        methods GET POST DELETE
        autoindex on
        upload_store ./www/uploads
    }

    location /cgi-bin {
        root ./www/cgi-bin
        methods GET POST
        cgi .py /usr/bin/python3
        cgi .sh /bin/sh
    }

    location /old {
        return 301 /
    }
}
```

Supported server directives:

- `listen host:port`
- `listen port`
- `host` or `server_address`
- `server_name`
- `client_max_body_size`
- `error_page`

Supported route directives:

- `root`
- `index`
- `methods`
- `return` or `redirect`
- `cgi`
- `autoindex`
- `upload_store`

Routes are matched by longest path prefix on path-segment boundaries. Paths with
`..` traversal are rejected with `403`.

Duplicate `listen` directives inside the same `server` block are rejected as a
configuration error. Multiple `server` blocks may still share the same
`host:port` for virtual hosting; the first server is the default if no
`server_name` matches.

## HTTP Behavior

The server parses HTTP requests incrementally and supports:

- Request line and headers.
- `Content-Length` bodies.
- Chunked request bodies.
- HTTP/1.0 fallback.
- HTTP/1.1 keep-alive.
- Correct status codes for success and error responses.

Unsupported methods return `501`. Methods that exist but are not allowed on a
route return `405` with an `Allow` header.

## CGI

CGI scripts are selected by file extension:

```nginx
cgi .py /usr/bin/python3
cgi .sh /bin/sh
```

The server runs CGI with:

- `fork`
- `pipe`
- `dup2`
- `execve`

The script path is passed as the first argument. The request body is sent to the
CGI process through stdin, and EOF marks the end of the body. The CGI stdout is
parsed as CGI headers followed by a blank line and a response body.

The CGI working directory is the script directory. Environment variables include:

- `GATEWAY_INTERFACE`
- `SERVER_PROTOCOL`
- `SERVER_SOFTWARE`
- `SERVER_NAME`
- `SERVER_PORT`
- `REMOTE_ADDR`
- `REQUEST_METHOD`
- `REQUEST_URI`
- `SCRIPT_NAME`
- `SCRIPT_FILENAME`
- `PATH_INFO`
- `QUERY_STRING`
- `CONTENT_TYPE`
- `CONTENT_LENGTH`
- `REDIRECT_STATUS`
- `HTTP_*` headers

Example CGI files:

- `www/cgi-bin/hello.py`
- `www/cgi-bin/env.sh`

## Architecture

- `src/main.rs`: program entry point, config loading, server startup.
- `src/server/epoll.rs`: thin wrapper around `epoll_create1`, `epoll_ctl`, and
  `epoll_wait`.
- `src/server/mod.rs`: main event loop, accept/read/write dispatch, timeouts,
  CGI pipe handling, and connection cleanup.
- `src/server/socket.rs`: raw socket creation, bind, listen, accept.
- `src/server/io_util.rs`: raw non-blocking read/write helpers.
- `src/server/cgi.rs`: CGI process management.
- `src/http/request.rs`: incremental HTTP request parser.
- `src/http/response.rs`: HTTP response serialization.
- `src/handler`: routes, static files, uploads, sessions, error pages,
  directory listings, MIME types, and path resolution.
- `src/config`: configuration parser and types.

## Tests

Run unit tests:

```sh
cargo test
```

Run integration tests:

```sh
bash tests/integration.sh
```

The integration suite covers:

- Static files.
- MIME headers.
- Directory redirects.
- Directory listing.
- Custom error pages.
- Wrong methods.
- Path traversal rejection.
- Multipart uploads.
- Raw uploads.
- `DELETE`.
- Body size limit.
- Chunked request bodies.
- Python CGI.
- Shell CGI.
- Sessions and cookies.
- Virtual hosts.
- HTTP/1.0.
- Keep-alive.

## Manual Audit Commands

Build and run:

```sh
cargo build --release
./target/release/localhost conf/default.conf
```

Basic requests:

```sh
curl -i http://127.0.0.1:8080/
curl -i http://127.0.0.1:8080/does-not-exist
curl -i http://127.0.0.1:8080/uploads/
curl -i http://127.0.0.1:8080/old
```

Upload and download:

```sh
printf 'hello upload\n' > /tmp/upload.txt
curl -i -X POST --data-binary @/tmp/upload.txt http://127.0.0.1:8080/uploads/upload.txt
curl -i http://127.0.0.1:8080/uploads/upload.txt
curl -i -X DELETE http://127.0.0.1:8080/uploads/upload.txt
```

Chunked upload:

```sh
curl -i -X POST -H "Transfer-Encoding: chunked" \
  --data-binary "chunked body" \
  http://127.0.0.1:8080/uploads/chunked.txt
```

CGI:

```sh
curl -i http://127.0.0.1:8080/cgi-bin/hello.py
curl -i http://127.0.0.1:8080/cgi-bin/env.sh
curl -i -X POST -d "ping=pong" http://127.0.0.1:8080/cgi-bin/hello.py
```

Session:

```sh
curl -i http://127.0.0.1:8080/session
```

Virtual host with a shared port:

```sh
curl -i -H "Host: one.test" http://127.0.0.1:8090/
curl -i -H "Host: two.test" http://127.0.0.1:8090/
```

## Stress Test

Install siege on WSL/Ubuntu:

```sh
sudo apt update
sudo apt install siege
```

Run:

```sh
siege -b http://127.0.0.1:8080/
```

The project requirement expects availability to stay above `99.5%`.

## Memory Test

Install Valgrind on WSL/Ubuntu:

```sh
sudo apt update
sudo apt install valgrind
```

Run:

```sh
valgrind --leak-check=full --show-leak-kinds=definite,indirect \
  --errors-for-leak-kinds=definite,indirect \
  ./target/release/localhost conf/default.conf
```

Stop the server with `Ctrl-C` after checking the Valgrind summary.
