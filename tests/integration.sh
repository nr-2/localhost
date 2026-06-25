#!/usr/bin/env bash
# Exhaustive curl-based integration test suite for the `localhost` server.
#
# Builds the release binary, starts it against conf/default.conf, exercises
# the HTTP feature surface (static files, directory listing, redirects,
# error pages, uploads, DELETE, CGI, sessions, chunked bodies, virtual
# hosts, body size limits, method validation, path traversal) and reports a
# pass/fail summary. Exits non-zero if any check fails.

set -uo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

BIN="$ROOT_DIR/target/release/localhost"
CONF="$ROOT_DIR/conf/default.conf"
LOG="/tmp/localhost_integration.log"
BASE1="http://127.0.0.1:8080"
BASE2="http://127.0.0.1:8081"

PASS=0
FAIL=0

pass() { PASS=$((PASS + 1)); echo "  PASS: $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  FAIL: $1"; }

check_eq() {
    local name="$1" expected="$2" actual="$3"
    if [ "$expected" = "$actual" ]; then
        pass "$name (got '$actual')"
    else
        fail "$name (expected '$expected', got '$actual')"
    fi
}

check_contains() {
    local name="$1" haystack="$2" needle="$3"
    if printf '%s' "$haystack" | grep -qF "$needle"; then
        pass "$name"
    else
        fail "$name (did not find '$needle')"
    fi
}

status_of() {
    curl -s -o /dev/null -w "%{http_code}" "$@"
}

# --------------------------------------------------------------------------
# Build & start server
# --------------------------------------------------------------------------

echo "Building release binary..."
if ! cargo build --release >>"$LOG" 2>&1; then
    echo "Build failed, see $LOG"
    exit 1
fi

echo "Stopping any existing server instances..."
pkill -f "$BIN" 2>/dev/null || true
sleep 0.3

echo "Starting server..."
: >"$LOG"
"$BIN" "$CONF" >>"$LOG" 2>&1 &
SERVER_PID=$!

cleanup() {
    kill "$SERVER_PID" 2>/dev/null || true
    rm -f "$ROOT_DIR/www/uploads/it-"*.txt 2>/dev/null || true
}
trap cleanup EXIT

# Wait for the server to report it is listening on both ports.
for _ in $(seq 1 50); do
    if grep -q "listening on 127.0.0.1:8080" "$LOG" && grep -q "listening on 127.0.0.1:8081" "$LOG"; then
        break
    fi
    sleep 0.1
done

if ! kill -0 "$SERVER_PID" 2>/dev/null; then
    echo "Server failed to start, log:"
    cat "$LOG"
    exit 1
fi

echo "Server is up (pid $SERVER_PID). Running tests..."
echo

# --------------------------------------------------------------------------
# Static files
# --------------------------------------------------------------------------

echo "== Static files =="
check_eq "GET / returns 200" "200" "$(status_of "$BASE1/")"
check_eq "GET /about.html returns 200" "200" "$(status_of "$BASE1/about.html")"
check_eq "GET /style.css returns 200" "200" "$(status_of "$BASE1/style.css")"
check_contains "GET /style.css has CSS content-type" "$(curl -s -i "$BASE1/style.css" | tr -d '\r')" "Content-Type: text/css"

# --------------------------------------------------------------------------
# Directory listing & redirects
# --------------------------------------------------------------------------

echo
echo "== Directory listing & redirects =="
check_eq "GET /uploads (no slash) redirects" "301" "$(status_of "$BASE1/uploads")"
check_contains "GET /uploads redirects to /uploads/" "$(curl -s -i "$BASE1/uploads" | tr -d '\r')" "Location: /uploads/"
check_eq "GET /uploads/ (autoindex) returns 200" "200" "$(status_of "$BASE1/uploads/")"
check_eq "GET /old returns 301" "301" "$(status_of "$BASE1/old")"
check_contains "GET /old redirects to /" "$(curl -s -i "$BASE1/old" | tr -d '\r')" "Location: /"

# --------------------------------------------------------------------------
# Error pages
# --------------------------------------------------------------------------

echo
echo "== Error pages =="
check_eq "GET /does-not-exist returns 404" "404" "$(status_of "$BASE1/does-not-exist")"
check_contains "404 page uses custom error page" "$(curl -s "$BASE1/does-not-exist")" "404"
check_eq "PUT / returns 501 (unsupported method)" "501" "$(status_of -X PUT "$BASE1/")"
check_eq "DELETE /cgi-bin/hello.py returns 405" "405" "$(status_of -X DELETE "$BASE1/cgi-bin/hello.py")"
check_contains "405 response has Allow header" "$(curl -s -i -X DELETE "$BASE1/cgi-bin/hello.py" | tr -d '\r')" "Allow:"

# --------------------------------------------------------------------------
# Path traversal
# --------------------------------------------------------------------------

echo
echo "== Path traversal =="
check_eq "GET /../etc/passwd is rejected" "403" "$(status_of --path-as-is "$BASE1/../etc/passwd")"
check_eq "GET /%2e%2e/%2e%2e/etc/passwd is rejected" "403" "$(status_of "$BASE1/%2e%2e/%2e%2e/etc/passwd")"

# --------------------------------------------------------------------------
# Uploads (multipart + raw) and DELETE
# --------------------------------------------------------------------------

echo
echo "== Uploads & DELETE =="
TMPFILE=$(mktemp)
echo "integration test payload" >"$TMPFILE"

UPLOAD_NAME="it-multipart-$$.txt"
cp "$TMPFILE" "/tmp/$UPLOAD_NAME"
RESP=$(curl -s -X POST -F "file=@/tmp/$UPLOAD_NAME" "$BASE1/uploads")
check_contains "multipart upload reports filename" "$RESP" "$UPLOAD_NAME"
check_eq "uploaded file exists on disk" "yes" "$( [ -f "$ROOT_DIR/www/uploads/$UPLOAD_NAME" ] && echo yes || echo no )"
check_eq "GET uploaded file returns 200" "200" "$(status_of "$BASE1/uploads/$UPLOAD_NAME")"
check_contains "GET uploaded file returns correct content" "$(curl -s "$BASE1/uploads/$UPLOAD_NAME")" "integration test payload"
check_eq "DELETE uploaded file returns 204" "204" "$(status_of -X DELETE "$BASE1/uploads/$UPLOAD_NAME")"
check_eq "DELETE again returns 404" "404" "$(status_of -X DELETE "$BASE1/uploads/$UPLOAD_NAME")"
check_eq "GET deleted file returns 404" "404" "$(status_of "$BASE1/uploads/$UPLOAD_NAME")"
rm -f "/tmp/$UPLOAD_NAME"

RAW_NAME="it-raw-$$.txt"
RESP=$(curl -s -X POST --data-binary @"$TMPFILE" "$BASE1/uploads/$RAW_NAME")
check_contains "raw upload reports bytes saved" "$RESP" "Saved"
check_contains "raw upload content matches" "$(curl -s "$BASE1/uploads/$RAW_NAME")" "integration test payload"
curl -s -o /dev/null -X DELETE "$BASE1/uploads/$RAW_NAME"
rm -f "$TMPFILE"

# --------------------------------------------------------------------------
# Body size limit (client_max_body_size 5M)
# --------------------------------------------------------------------------

echo
echo "== Body size limit =="
BIGFILE=$(mktemp)
dd if=/dev/zero of="$BIGFILE" bs=1M count=6 status=none
check_eq "POST body over limit returns 413" "413" "$(status_of -X POST --data-binary @"$BIGFILE" "$BASE1/uploads")"
rm -f "$BIGFILE"

# --------------------------------------------------------------------------
# Chunked transfer encoding
# --------------------------------------------------------------------------

echo
echo "== Chunked transfer encoding =="
CHUNK_NAME="it-chunk-$$.txt"
RESP=$(curl -s -X POST -H "Transfer-Encoding: chunked" --data-binary "chunked-body-data" "$BASE1/uploads/$CHUNK_NAME")
check_contains "chunked upload accepted" "$RESP" "Saved"
check_contains "chunked upload content matches" "$(curl -s "$BASE1/uploads/$CHUNK_NAME")" "chunked-body-data"
curl -s -o /dev/null -X DELETE "$BASE1/uploads/$CHUNK_NAME"

# --------------------------------------------------------------------------
# CGI
# --------------------------------------------------------------------------

echo
echo "== CGI =="
check_eq "GET /cgi-bin/hello.py returns 200" "200" "$(status_of "$BASE1/cgi-bin/hello.py")"
check_contains "hello.py CGI output looks right" "$(curl -s "$BASE1/cgi-bin/hello.py")" "Hello from Python CGI"
check_eq "GET /cgi-bin/env.sh returns 200" "200" "$(status_of "$BASE1/cgi-bin/env.sh")"
check_contains "env.sh CGI output looks right" "$(curl -s "$BASE1/cgi-bin/env.sh")" "env.sh - Shell CGI"
RESP=$(curl -s -X POST -d "ping=pong" "$BASE1/cgi-bin/hello.py")
check_contains "POST body reaches CGI" "$RESP" "ping=pong"
check_contains "POST method reflected in CGI env" "$RESP" "POST"

# --------------------------------------------------------------------------
# Cookies & sessions
# --------------------------------------------------------------------------

echo
echo "== Sessions =="
HEADERS=$(curl -s -i "$BASE1/session" | tr -d '\r')
check_contains "/session sets a session_id cookie" "$HEADERS" "Set-Cookie: session_id="
check_contains "/session first visit count is 1" "$HEADERS" "Visits recorded in this session: <strong>1</strong>"

SID=$(printf '%s\n' "$HEADERS" | grep -i '^Set-Cookie:' | sed -n 's/.*session_id=\([^;]*\).*/\1/p')
RESP2=$(curl -s --cookie "session_id=$SID" "$BASE1/session")
check_contains "/session second visit count is 2" "$RESP2" "Visits recorded in this session: <strong>2</strong>"

# --------------------------------------------------------------------------
# Virtual hosts
# --------------------------------------------------------------------------

echo
echo "== Virtual hosts =="
check_eq "GET 127.0.0.1:8081/ returns 200" "200" "$(status_of "$BASE2/")"
check_eq "second server only allows GET (POST -> 405)" "405" "$(status_of -X POST "$BASE2/")"

# --------------------------------------------------------------------------
# HTTP/1.0 and keep-alive
# --------------------------------------------------------------------------

echo
echo "== Protocol handling =="
check_eq "HTTP/1.0 request works" "200" "$(curl -s -o /dev/null -w '%{http_code}' --http1.0 "$BASE1/")"
KEEPALIVE_CODES=$(curl -s -o /dev/null -w '%{http_code}\n' "$BASE1/" -o /dev/null -w '%{http_code}\n' "$BASE1/about.html")
check_eq "keep-alive: two requests on one connection" "$(printf '200\n200')" "$KEEPALIVE_CODES"

# --------------------------------------------------------------------------
# Summary
# --------------------------------------------------------------------------

echo
echo "============================================"
echo "Passed: $PASS   Failed: $FAIL"
echo "============================================"

if [ "$FAIL" -ne 0 ]; then
    echo "Server log:"
    cat "$LOG"
    exit 1
fi
exit 0
