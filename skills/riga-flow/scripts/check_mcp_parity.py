#!/usr/bin/env python3
"""MCP transport parity check: stdio vs Streamable HTTP (handoff gate 4).

Runs `riga-mcp` over stdio and over HTTP, issues the same `price_put` call to each,
and asserts the structured payloads are identical. See ../SKILL.md.

Usage: python3 check_mcp_parity.py   (requires `cargo build --release -p riga-mcp`)
"""
import json
import os
import subprocess
import sys
import time
import urllib.request

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))
BIN = os.path.join(ROOT, "target", "release", "riga-mcp")
PORT = 8799
BASE = f"http://127.0.0.1:{PORT}/mcp"

INIT = {"jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                   "clientInfo": {"name": "parity", "version": "0"}}}
CALL = {"jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": {"name": "price_put", "arguments": {}}}


def stdio_result():
    payload = json.dumps(INIT) + "\n" + json.dumps(
        {"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n" + json.dumps(CALL) + "\n"
    proc = subprocess.run([BIN, "--transport", "stdio"], input=payload.encode(),
                          capture_output=True, timeout=120)
    for line in proc.stdout.decode().splitlines():
        try:
            message = json.loads(line)
        except Exception:
            continue
        if message.get("id") == 2:
            return message["result"]["structuredContent"]
    raise RuntimeError(f"no stdio result: {proc.stderr.decode()[:300]}")


def http_result(proc):
    def post(body, session=None):
        request = urllib.request.Request(BASE, data=json.dumps(body).encode(), method="POST")
        request.add_header("Content-Type", "application/json")
        request.add_header("Accept", "application/json, text/event-stream")
        if session:
            request.add_header("Mcp-Session-Id", session)
        with urllib.request.urlopen(request, timeout=30) as response:
            return response.headers.get("Mcp-Session-Id"), response.read().decode()

    # Initialize (capture the session id from the response header).
    session = None
    for _ in range(60):
        try:
            session, _ = post(INIT)
            break
        except Exception:
            time.sleep(0.25)
    post({"jsonrpc": "2.0", "method": "notifications/initialized"}, session)
    _, body = post(CALL, session)
    text = body.strip()
    if text.startswith("event:") or text.startswith("data:"):
        data = [l[5:].strip() for l in text.splitlines() if l.startswith("data:")]
        text = data[-1] if data else "{}"
    return json.loads(text)["result"]["structuredContent"]


def main():
    if not os.path.exists(BIN):
        sys.exit(f"{BIN} not found — run: cargo build --release -p riga-mcp")
    stdio = stdio_result()

    server = subprocess.Popen(
        [BIN, "--transport", "http", "--bind", f"127.0.0.1:{PORT}",
         "--allowed-host", "localhost", "--allowed-host", "127.0.0.1"],
        stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
    )
    try:
        http = http_result(server)
    finally:
        server.terminate()
        server.wait(timeout=10)

    key = lambda r: json.dumps(r, sort_keys=True)
    if key(stdio) == key(http):
        print("MCP parity: PASS (stdio == Streamable HTTP)")
        print(f"  PUT = {stdio['result']['legs']['down_and_in_put']:.6f}, acceptance = {stdio['fixture_acceptance']['status']}")
    else:
        print("MCP parity: FAIL")
        print("  stdio:", key(stdio)[:300])
        print("  http :", key(http)[:300])
        sys.exit(1)


if __name__ == "__main__":
    main()
