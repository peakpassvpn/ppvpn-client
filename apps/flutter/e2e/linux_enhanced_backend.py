#!/usr/bin/env python3
"""The backend of the Flutter app's Linux enhanced-mode end-to-end test
(#401 step 2, linux_enhanced.sh): the routes crates/desktop's Client calls
to restore a saved sign-in (the refresh endpoints), load the profile and
register the device, with the shapes of crates/desktop/src/test_backend.rs.
Standard library only; serves http on 127.0.0.1, which the client accepts
only with PPVPN_TEST_TRUST_LOCAL_BACKEND=1 (crates/desktop/src/testmode.rs).

The profile is crates/desktop/src/e2e_linux.rs's: one Shadowsocks 2022 node
at test/fakenode in run.sh's uplink namespace (10.243.0.2:8388, given by
domain so that the core's public-address check does not apply), private
destinations (this backend on loopback) direct, the rest through the node.

Usage: linux_enhanced_backend.py --port N [--log FILE]
Prints `ready http://127.0.0.1:N` once it listens.
"""

import argparse
import base64
import json
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlsplit

NODE_IP = "10.243.0.2"
NODE_PORT = 8388
NODE_KEY = "AAAAAAAAAAAAAAAAAAAAAA=="  # SS2022 aes-128, as e2e_linux.rs


def b64url(data):
    return base64.urlsafe_b64encode(json.dumps(data).encode()).rstrip(b"=").decode()


def token_set(n):
    # The client reads only the audience; the signature is never checked.
    jwt = f"{b64url({'alg': 'none'})}.{b64url({'aud': ['ppvpn'], 'sub': 'u7'})}.sig"
    return {"access_token": jwt, "refresh_token": f"jyr_e2e_refresh_{n:04d}_" + "x" * 32, "expires_in": 900}


PROFILE = {
    "schema_version": 1,
    "revision": "e2e" + "0" * 60 + "1",
    "generated_at": "2026-01-01T00:00:00Z",
    "expires_at": "2099-01-01T00:00:00Z",
    "nodes": [{
        "id": "e2e-node",
        "name": "e2e",
        "entry_key": "standard",
        "exit": {"ip": "203.0.113.10", "region": "e2e"},
        "capabilities": {"tcp": True, "udp": True},
        "ingresses": [{
            "role": "primary",
            "endpoint_key": "e2e-1",
            "replica_ordinal": 0,
            "protocol": "shadowsocks",
            "endpoint": {"domain": NODE_IP, "port": NODE_PORT},
            "credentials": {"shadowsocks": {"method": "2022-blake3-aes-128-gcm", "user_key": NODE_KEY}},
            "capabilities": {"tcp": True, "udp": True},
        }],
    }],
    "selection": {"mode": "manual", "default_node_id": "e2e-node"},
    "routing": {
        "rules": [{"id": "bypass-private", "match": {"ip_is_private": True}, "action": {"type": "direct"}}],
        "final": {"type": "proxy", "target": "selected"},
    },
}


class State:
    def __init__(self):
        self.tokens = 0
        self.lock = threading.Lock()


def handler(state, log):
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, fmt, *args):
            if log:
                log.write(f"{self.command} {self.path} -> {args[1] if len(args) > 1 else ''}\n")
                log.flush()

        def reply(self, status, body):
            data = b"" if body is None else json.dumps(body).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            self.wfile.write(data)

        def body(self):
            length = int(self.headers.get("Content-Length") or 0)
            raw = self.rfile.read(length) if length else b""
            try:
                return json.loads(raw or b"{}")
            except ValueError:
                return {}

        def next_tokens(self):
            with state.lock:
                state.tokens += 1
                return token_set(state.tokens)

        def do_GET(self):
            url = urlsplit(self.path)
            routes = {
                "/api/v1/health": {"status": "ok"},
                "/api/v1/users/me": {"id": "u7", "name": "E2E", "avatar": ""},
                "/api/v1/me/teams": {"items": [{"id": "t1", "name": "CI Team", "is_personal": True, "is_default": True}]},
                "/api/v1/me/proxy-profile": PROFILE,
                "/api/v1/messages/unread-count": {"count": 0},
                "/api/v1/messages": {"items": [], "total": 0, "page": 1, "page_size": 20},
            }
            if url.path in routes:
                self.reply(200, routes[url.path])
            elif url.path == "/api/v1/push/pull":
                # Nothing to push: the long poll ends empty (at most 5 s here).
                query = parse_qs(url.query)
                after = int((query.get("after") or ["0"])[0] or 0)
                time.sleep(min(int((query.get("wait") or ["0"])[0] or 0), 5))
                self.reply(200, {"items": [], "cursor": after})
            else:
                self.reply(404, {"code": "NOT_FOUND"})

        def do_POST(self):
            path = urlsplit(self.path).path
            request = self.body()
            if path in ("/api/v1/auth/device/activate", "/api/v1/auth/device/refresh/commit"):
                self.reply(200, self.next_tokens())
            elif path == "/api/v1/auth/device/refresh":
                self.reply(200, {"refresh_token": "jyr_e2e_prepared_" + "z" * 32})
            elif path == "/api/v1/auth/device/revoke":
                self.reply(200, {"revoked": True})
            elif path == "/api/v1/devices/register":
                self.reply(200, {"id": 42, "platform": request.get("platform"), "push_token": "ppd_e2e"})
            elif path == "/api/v1/push/ack":
                self.reply(200, {})
            else:
                self.reply(404, {"code": "NOT_FOUND"})

        def do_PUT(self):
            self.reply(200, {"updated": True})

        def do_DELETE(self):
            self.reply(204, None)

    return Handler


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--log")
    args = parser.parse_args()
    log = open(args.log, "a", encoding="utf-8") if args.log else None
    server = ThreadingHTTPServer(("127.0.0.1", args.port), handler(State(), log))
    server.daemon_threads = True
    print(f"ready http://127.0.0.1:{args.port}", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    return 0


if __name__ == "__main__":
    sys.exit(main())
