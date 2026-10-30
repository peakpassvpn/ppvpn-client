"""play_upload.py: its assertion verifies with the key's public half, and one
upload is one committed edit against a fake Play API."""

import base64
import http.server
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import threading
import unittest
import urllib.parse
from unittest import mock

sys.path.insert(0, str(pathlib.Path(__file__).parent))
import play_upload  # noqa: E402


def unb64url(text):
    return base64.urlsafe_b64decode(text + "=" * (-len(text) % 4))


class Keys:
    def __init__(self):
        self.dir = tempfile.TemporaryDirectory()
        self.key = pathlib.Path(self.dir.name, "key.pem")
        subprocess.run(["openssl", "genpkey", "-algorithm", "RSA", "-pkeyopt", "rsa_keygen_bits:2048",
                        "-out", str(self.key)], check=True, capture_output=True)
        self.pub = pathlib.Path(self.dir.name, "key.pub")
        subprocess.run(["openssl", "pkey", "-in", str(self.key), "-pubout", "-out", str(self.pub)],
                       check=True, capture_output=True)

    def account(self, token_uri):
        return {"client_email": "uploader@example.iam.gserviceaccount.com",
                "private_key": self.key.read_text(), "token_uri": token_uri}

    def verifies(self, jwt):
        header, payload, signature = jwt.split(".")
        sig = pathlib.Path(self.dir.name, "sig")
        sig.write_bytes(unb64url(signature))
        result = subprocess.run(["openssl", "dgst", "-sha256", "-verify", str(self.pub), "-signature", str(sig)],
                                input=f"{header}.{payload}".encode(), capture_output=True)
        return result.returncode == 0

    def cleanup(self):
        self.dir.cleanup()


class AssertionTest(unittest.TestCase):
    def setUp(self):
        self.keys = Keys()

    def tearDown(self):
        self.keys.cleanup()

    def test_assertion_verifies(self):
        jwt = play_upload.assertion(self.keys.account("https://oauth2.example/token"), now=1000)
        header, payload, _ = jwt.split(".")
        self.assertEqual(json.loads(unb64url(header)), {"alg": "RS256", "typ": "JWT"})
        self.assertEqual(json.loads(unb64url(payload)), {
            "iss": "uploader@example.iam.gserviceaccount.com", "scope": play_upload.SCOPE,
            "aud": "https://oauth2.example/token", "iat": 1000, "exp": 1600})
        self.assertTrue(self.keys.verifies(jwt))


class FakePlay(http.server.BaseHTTPRequestHandler):
    """Token endpoint and the four edit calls; records each (method, path, body)."""

    calls = []
    keys = None

    def log_message(self, *args):
        pass

    def reply(self, code, body):
        data = json.dumps(body).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def handle_any(self):
        length = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(length)
        path = urllib.parse.urlsplit(self.path).path
        FakePlay.calls.append((self.command, self.path, body, self.headers.get("Authorization")))
        prefix = "/androidpublisher/v3/applications/com.example.app/edits"
        if path == "/token":
            form = urllib.parse.parse_qs(body.decode())
            if not FakePlay.keys.verifies(form["assertion"][0]):
                return self.reply(401, {"error": "invalid_grant"})
            return self.reply(200, {"access_token": "tok", "expires_in": 3600})
        if self.headers.get("Authorization") != "Bearer tok":
            return self.reply(401, {"error": "unauthenticated"})
        if self.command == "POST" and path == prefix:
            return self.reply(200, {"id": "e1"})
        if self.command == "POST" and path == "/upload" + prefix + "/e1/bundles":
            return self.reply(200, {"versionCode": 4321, "sha256": "x"})
        if self.command == "PUT" and path == prefix + "/e1/tracks/internal":
            return self.reply(200, json.loads(body))
        if self.command == "POST" and path == prefix + "/e1:commit":
            return self.reply(200, {"id": "e1"})
        return self.reply(404, {"error": {"message": f"no {self.command} {path}"}})

    do_GET = do_POST = do_PUT = handle_any


class UploadTest(unittest.TestCase):
    def setUp(self):
        self.keys = Keys()
        FakePlay.keys = self.keys
        FakePlay.calls = []
        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), FakePlay)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.base = f"http://127.0.0.1:{self.server.server_port}"
        self.bundle = pathlib.Path(self.keys.dir.name, "app-release.aab")
        self.bundle.write_bytes(b"PK fake bundle")

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()
        self.keys.cleanup()

    def run_main(self, status=None):
        env = {"PLAY_SERVICE_ACCOUNT_JSON": json.dumps(self.keys.account(f"{self.base}/token"))}
        if status is not None:
            env["PLAY_RELEASE_STATUS"] = status
        with mock.patch.dict(os.environ, env), mock.patch.object(play_upload, "API", self.base):
            play_upload.main(["com.example.app", str(self.bundle), "internal", "0.7.2 (300)"])

    def test_one_committed_edit(self):
        self.run_main()
        steps = [(method, urllib.parse.urlsplit(path).path) for method, path, _, _ in FakePlay.calls]
        prefix = "/androidpublisher/v3/applications/com.example.app/edits"
        self.assertEqual(steps, [
            ("POST", "/token"),
            ("POST", prefix),
            ("POST", "/upload" + prefix + "/e1/bundles"),
            ("PUT", prefix + "/e1/tracks/internal"),
            ("POST", prefix + "/e1:commit"),
        ])
        self.assertEqual(FakePlay.calls[2][2], b"PK fake bundle")
        self.assertIn("uploadType=media", FakePlay.calls[2][1])
        self.assertEqual(json.loads(FakePlay.calls[3][2]), {"track": "internal", "releases": [
            {"name": "0.7.2 (300)", "versionCodes": ["4321"], "status": "completed"}]})

    def test_draft_status(self):
        self.run_main(status="draft")
        self.assertEqual(json.loads(FakePlay.calls[3][2])["releases"][0]["status"], "draft")

    def test_unknown_status_refused_before_any_call(self):
        with self.assertRaises(SystemExit):
            self.run_main(status="inProgress")
        self.assertEqual(FakePlay.calls, [])

    def test_http_error_names_the_step(self):
        env = {"PLAY_SERVICE_ACCOUNT_JSON": json.dumps(self.keys.account(f"{self.base}/token"))}
        with mock.patch.dict(os.environ, env), mock.patch.object(play_upload, "API", self.base):
            with self.assertRaises(SystemExit) as caught:
                play_upload.main(["com.example.other", str(self.bundle), "internal", "x"])
        self.assertIn("edit: HTTP 404", str(caught.exception))

    def test_not_a_bundle(self):
        with self.assertRaises(SystemExit):
            play_upload.main(["com.example.app", str(self.bundle.with_suffix(".apk")), "internal", "x"])


if __name__ == "__main__":
    unittest.main()
