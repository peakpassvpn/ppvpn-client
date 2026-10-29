"""asc_profiles.py: its token verifies with the key's public half, and profiles
come from a fake API by exact name."""

import base64
import http.server
import json
import pathlib
import subprocess
import sys
import tempfile
import threading
import unittest
import urllib.parse

sys.path.insert(0, str(pathlib.Path(__file__).parent))
import asc_profiles  # noqa: E402


def unb64url(text):
    return base64.urlsafe_b64decode(text + "=" * (-len(text) % 4))


def raw_to_der(raw):
    def integer(value):
        value = value.lstrip(b"\0") or b"\0"
        if value[0] & 0x80:
            value = b"\0" + value
        return b"\x02" + bytes([len(value)]) + value
    body = integer(raw[:32]) + integer(raw[32:])
    return b"\x30" + bytes([len(body)]) + body


class TokenTest(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.key = pathlib.Path(self.dir.name, "key.p8")
        subprocess.run(["openssl", "genpkey", "-algorithm", "EC", "-pkeyopt", "ec_paramgen_curve:P-256",
                        "-out", str(self.key)], check=True, capture_output=True)
        self.pub = pathlib.Path(self.dir.name, "key.pub")
        subprocess.run(["openssl", "pkey", "-in", str(self.key), "-pubout", "-out", str(self.pub)],
                       check=True, capture_output=True)

    def tearDown(self):
        self.dir.cleanup()

    def test_token_verifies(self):
        jwt = asc_profiles.token("KEY123", "issuer-id", self.key.read_text(), now=1000)
        header, payload, signature = jwt.split(".")
        self.assertEqual(json.loads(unb64url(header)), {"alg": "ES256", "kid": "KEY123", "typ": "JWT"})
        self.assertEqual(json.loads(unb64url(payload)),
                         {"iss": "issuer-id", "iat": 1000, "exp": 1600, "aud": "appstoreconnect-v1"})
        raw = unb64url(signature)
        self.assertEqual(len(raw), 64)
        sig = pathlib.Path(self.dir.name, "sig.der")
        sig.write_bytes(raw_to_der(raw))
        verified = subprocess.run(["openssl", "dgst", "-sha256", "-verify", str(self.pub), "-signature", str(sig)],
                                  input=f"{header}.{payload}".encode(), capture_output=True)
        self.assertEqual(verified.returncode, 0, verified.stdout + verified.stderr)

    def test_der_to_raw_pads_and_strips(self):
        r, s = b"\x01" * 31, b"\xff" + b"\x02" * 31
        self.assertEqual(asc_profiles.der_to_raw(raw_to_der(b"\0" + r + s)), b"\0" + r + s)
        with self.assertRaises(ValueError):
            asc_profiles.der_to_raw(b"\x31\x06\x02\x01\x01\x02\x01\x01")


PROFILE = {"name": "PPVPN iOS App Store", "uuid": "AAAA-1", "expirationDate": "2027-10-28",
           "profileContent": base64.b64encode(b"app profile").decode()}


class FakeAPI(http.server.BaseHTTPRequestHandler):
    """The four calls asc_profiles.py makes; FakeAPI.profiles is the account's."""
    profiles = []
    requests = []

    def reply(self, data):
        body = json.dumps({"data": data}).encode()
        self.send_response(201 if self.command == "POST" else 200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        url = urllib.parse.urlparse(self.path)
        query = {k: v[0] for k, v in urllib.parse.parse_qs(url.query).items()}
        FakeAPI.requests.append(("GET", url.path, query, self.headers.get("Authorization")))
        if url.path == "/v1/profiles":
            self.reply([{"attributes": p} for p in FakeAPI.profiles if query["filter[name]"] in p["name"]])
        elif url.path == "/v1/bundleIds":
            ids = [("B1", "com.peakpassvpn.ppvpn"), ("B2", "com.peakpassvpn.ppvpn.tunnel")]
            self.reply([{"id": i, "attributes": {"identifier": b, "platform": "IOS"}}
                        for i, b in ids if b.startswith(query["filter[identifier]"])])
        elif url.path == "/v1/certificates":
            self.reply([{"id": "C-old", "attributes": {"serialNumber": "11AA"}},
                        {"id": "C1", "attributes": {"serialNumber": "7F00BEEF"}}])
        else:
            self.send_error(404)

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        FakeAPI.requests.append(("POST", self.path, body, self.headers.get("Authorization")))
        attributes = dict(PROFILE, name=body["data"]["attributes"]["name"], uuid="NEW-1")
        FakeAPI.profiles.append(attributes)
        self.reply({"attributes": attributes})

    def log_message(self, *args):
        pass


class ProfileTest(unittest.TestCase):
    def setUp(self):
        self.server = http.server.HTTPServer(("127.0.0.1", 0), FakeAPI)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.api, asc_profiles.API = asc_profiles.API, f"http://127.0.0.1:{self.server.server_port}"
        FakeAPI.requests = []
        FakeAPI.profiles = [PROFILE, dict(PROFILE, name="PPVPN iOS App Store Old", uuid="BBBB-2")]

    def tearDown(self):
        asc_profiles.API = self.api
        self.server.shutdown()
        self.server.server_close()

    def test_exact_name_installed_in_each_dir(self):
        attributes = asc_profiles.find_profile("PPVPN iOS App Store", "token")
        self.assertEqual(attributes["uuid"], "AAAA-1")
        method, path, query, auth = FakeAPI.requests[0]
        self.assertEqual((method, path), ("GET", "/v1/profiles"))
        self.assertEqual(query["filter[profileState]"], "ACTIVE")
        self.assertEqual(query["filter[profileType]"], "IOS_APP_STORE")
        self.assertEqual(auth, "Bearer token")
        with tempfile.TemporaryDirectory() as home:
            dirs = [pathlib.Path(home, "a"), pathlib.Path(home, "b")]
            asc_profiles.install(attributes, dirs)
            for directory in dirs:
                self.assertEqual((directory / "AAAA-1.mobileprovision").read_bytes(), b"app profile")

    def test_missing_profile_is_none(self):
        self.assertIsNone(asc_profiles.find_profile("PPVPN iOS Tunnel App Store", "token"))

    def test_create_on_the_exact_bundle_and_certificate(self):
        attributes = asc_profiles.create_profile("PPVPN iOS Tunnel App Store", "com.peakpassvpn.ppvpn",
                                                 "007f00beef", "token")
        self.assertEqual(attributes["uuid"], "NEW-1")
        method, path, body, _ = FakeAPI.requests[-1]
        self.assertEqual((method, path), ("POST", "/v1/profiles"))
        self.assertEqual(body["data"]["attributes"], {"name": "PPVPN iOS Tunnel App Store", "profileType": "IOS_APP_STORE"})
        # com.peakpassvpn.ppvpn's prefix match also returns .tunnel; the exact one is B1.
        self.assertEqual(body["data"]["relationships"]["bundleId"]["data"], {"type": "bundleIds", "id": "B1"})
        self.assertEqual(body["data"]["relationships"]["certificates"]["data"], [{"type": "certificates", "id": "C1"}])

    def test_unknown_certificate_fails(self):
        with self.assertRaises(SystemExit):
            asc_profiles.create_profile("x", "com.peakpassvpn.ppvpn", "DEAD", "token")


if __name__ == "__main__":
    unittest.main()
