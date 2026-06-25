#!/bin/sh
# A minimal shell CGI script. Reads (and discards) the request body up to
# CONTENT_LENGTH bytes from stdin, then writes a CGI response.

if [ -n "$CONTENT_LENGTH" ] && [ "$CONTENT_LENGTH" -gt 0 ] 2>/dev/null; then
    dd bs=1 count="$CONTENT_LENGTH" of=/dev/null 2>/dev/null
fi

BODY=$(env | sort)

printf 'Status: 200 OK\r\n'
printf 'Content-Type: text/plain; charset=utf-8\r\n'
printf '\r\n'
echo "env.sh - Shell CGI"
echo "=================="
echo
echo "$BODY"
