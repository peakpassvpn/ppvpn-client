#!/usr/bin/env python3
"""The backend the Flutter app's end-to-end test signs in to (#401, step 1):
the routes crates/desktop's Client calls for device sign-in, the profile and
sign-out, with the shapes of crates/desktop/src/test_backend.rs. Standard
library only; serves http on 127.0.0.1, which the client accepts only with
PPVPN_TEST_TRUST_LOCAL_BACKEND=1 (crates/desktop/src/testmode.rs).

The device code is approved on the second poll. The profile has two nodes,
both on the local fakenode (test/fakenode), given by domain so that the
core's public-address check does not apply (as e2e_linux.rs does).

Usage: mock_backend.py --port N --node-port N [--log FILE]
Prints `ready http://127.0.0.1:N` once it listens.
"""

import argparse
import base64
import json
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

NODE_KEY = "AAAAAAAAAAAAAAAAAAAAAA=="  # SS2022 aes-128, as e2e_linux.rs
USER_CODE = "E2ET-CODE"


def b64url(data):
    return base64.urlsafe_b64encode(json.dumps(data).encode()).rstrip(b"=").decode()


def token_set(n):
    # The client reads only the audience; the signature is never checked.
    jwt = f"{b64url({'alg': 'none'})}.{b64url({'aud': ['ppvpn'], 'sub': 'u7'})}.sig"
    return {"access_token": jwt, "refresh_token": f"jyr_e2e_refresh_{n:04d}_" + "x" * 32, "expires_in": 900}


def profile(node_port):
    def node(node_id, name):
        return {
            "id": node_id,
            "name": name,
            "entry_key": "standard",
            "exit": {"ip": "203.0.113.10", "region": "ci"},
            "capabilities": {"tcp": True, "udp": True},
            "ingresses": [{
                "role": "primary",
                "endpoint_key": f"{node_id}-1",
                "replica_ordinal": 0,
                "protocol": "shadowsocks",
                "endpoint": {"domain": "127.0.0.1", "port": node_port},
                "credentials": {"shadowsocks": {"method": "2022-blake3-aes-128-gcm", "user_key": NODE_KEY}},
                "capabilities": {"tcp": True, "udp": True},
            }],
        }

    return {
        "schema_version": 1,
        "revision": "e2e" + "0" * 61,
        "generated_at": "2026-01-01T00:00:00Z",
        "expires_at": "2099-01-01T00:00:00Z",
        "nodes": [node("ci-one", "CI One"), node("ci-two", "CI Two")],
        "selection": {"mode": "manual", "default_node_id": "ci-one"},
        "routing": {"rules": [], "final": {"type": "proxy", "target": "selected"}},
    }


class State:
    def __init__(self, node_port):
        self.node_port = node_port
        self.polls = 0
        self.tokens = 0
        self.lock = threading.Lock()


def handler(state, base, log):
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
            path = self.path.split("?", 1)[0]
            routes = {
                "/api/v1/health": {"status": "ok"},
                "/api/v1/users/me": {"id": "u7", "name": "E2E", "avatar": ""},
                "/api/v1/me/teams": {"items": [{"id": "t1", "name": "CI Team", "is_personal": True, "is_default": True}]},
                "/api/v1/me/proxy-profile": profile(state.node_port),
                "/api/v1/messages/unread-count": {"count": 0},
                "/api/v1/messages": {"items": [], "total": 0, "page": 1, "page_size": 20},
            }
            if path in routes:
                self.reply(200, routes[path])
            else:
                self.reply(404, {"code": "NOT_FOUND"})

        def do_POST(self):
            path = self.path.split("?", 1)[0]
            request = self.body()
            if path == "/api/v1/auth/device/code":
                self.reply(200, {
                    "device_code": "e2e-device-code",
                    "user_code": USER_CODE,
                    "verification_uri_complete": f"{base}/dashboard/device/authorize?user_code={USER_CODE}",
                    "expires_in": 600,
                    "interval": 1,
                })
            elif path == "/api/v1/auth/device/token":
                with state.lock:
                    state.polls += 1
                    approved = state.polls >= 2
                if approved:
                    self.reply(200, {"status": "authorized", "refresh_token": "jyr_e2e_device_" + "y" * 40})
                else:
                    self.reply(200, {"status": "authorization_pending"})
            elif path in ("/api/v1/auth/device/activate", "/api/v1/auth/device/refresh/commit"):
                self.reply(200, self.next_tokens())
            elif path == "/api/v1/auth/device/refresh":
                self.reply(200, {"refresh_token": "jyr_e2e_prepared_" + "z" * 32})
            elif path == "/api/v1/auth/device/revoke":
                self.reply(200, {"revoked": True})
            elif path == "/api/v1/devices/register":
                self.reply(200, {"id": 42, "platform": request.get("platform"), "push_token": "ppd_e2e"})
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
    parser.add_argument("--node-port", type=int, required=True)
    parser.add_argument("--log")
    args = parser.parse_args()
    base = f"http://127.0.0.1:{args.port}"
    log = open(args.log, "a", encoding="utf-8") if args.log else None
    server = ThreadingHTTPServer(("127.0.0.1", args.port), handler(State(args.node_port), base, log))
    print(f"ready {base}", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    return 0


if __name__ == "__main__":
    sys.exit(main())
