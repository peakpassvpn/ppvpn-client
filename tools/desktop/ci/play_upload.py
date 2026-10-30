#!/usr/bin/env python3
"""Uploads an Android App Bundle to a Google Play track.

release.yml's android job (#448), with the release environment's service
account, after the approval:

    play_upload.py <package> <app.aab> <track> <release name>

The service account's JSON key comes from the environment in
PLAY_SERVICE_ACCOUNT_JSON; it needs release access to the app in Play
Console. One edit: the bundle uploaded, the track set to that version code
alone, the edit committed. The release's status is PLAY_RELEASE_STATUS
(default completed): an app that has never been published accepts only
draft releases, which then go out from Play Console. Play takes the first
bundle of a new app only through Play Console, by hand. Only the standard
library and openssl (for the RS256 signature) are used.
"""

import base64
import json
import os
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request

API = os.environ.get("PLAY_API_BASE", "https://androidpublisher.googleapis.com")
SCOPE = "https://www.googleapis.com/auth/androidpublisher"
STATUSES = ("completed", "draft")


def b64url(data):
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode()


def assertion(account, now=None):
    """The signed JWT a service account trades for an access token (RFC 7523)."""
    now = int(time.time() if now is None else now)
    header = {"alg": "RS256", "typ": "JWT"}
    payload = {"iss": account["client_email"], "scope": SCOPE, "aud": account["token_uri"],
               "iat": now, "exp": now + 600}
    signing_input = f"{b64url(json.dumps(header).encode())}.{b64url(json.dumps(payload).encode())}"
    # Created 0600.
    with tempfile.NamedTemporaryFile("w", suffix=".pem") as key:
        key.write(account["private_key"])
        key.flush()
        signature = subprocess.run(
            ["openssl", "dgst", "-sha256", "-sign", key.name],
            input=signing_input.encode(), capture_output=True, check=True,
        ).stdout
    return f"{signing_input}.{b64url(signature)}"


def request(method, url, headers=None, data=None, what=None):
    req = urllib.request.Request(url, data=data, headers=headers or {}, method=method)
    try:
        with urllib.request.urlopen(req, timeout=600) as response:
            return json.load(response)
    except urllib.error.HTTPError as error:
        path = urllib.parse.urlsplit(url).path
        raise SystemExit(f"{what or method + ' ' + path}: HTTP {error.code}: "
                         f"{error.read().decode(errors='replace')[:2000]}")


def access_token(account):
    body = urllib.parse.urlencode({
        "grant_type": "urn:ietf:params:oauth:grant-type:jwt-bearer",
        "assertion": assertion(account),
    }).encode()
    reply = request("POST", account["token_uri"], {"Content-Type": "application/x-www-form-urlencoded"},
                    body, what="token")
    return reply["access_token"]


def upload(package, bundle, track, name, status, bearer):
    """One edit: the bundle, the track at its version code, the commit. Returns the version code."""
    auth = {"Authorization": f"Bearer {bearer}"}
    base = f"{API}/androidpublisher/v3/applications/{urllib.parse.quote(package)}/edits"
    edit = request("POST", base, {**auth, "Content-Type": "application/json"}, b"{}", what="edit")["id"]
    with open(bundle, "rb") as file:
        data = file.read()
    uploaded = request(
        "POST",
        f"{API}/upload/androidpublisher/v3/applications/{urllib.parse.quote(package)}/edits/{edit}/bundles"
        "?uploadType=media",
        {**auth, "Content-Type": "application/octet-stream"}, data, what="bundle upload")
    code = int(uploaded["versionCode"])
    release = {"name": name, "versionCodes": [str(code)], "status": status}
    request("PUT", f"{base}/{edit}/tracks/{urllib.parse.quote(track)}",
            {**auth, "Content-Type": "application/json"},
            json.dumps({"track": track, "releases": [release]}).encode(), what=f"track {track}")
    request("POST", f"{base}/{edit}:commit", auth, b"", what="commit")
    return code


def main(args):
    if len(args) != 4:
        raise SystemExit("usage: play_upload.py <package> <app.aab> <track> <release name>")
    package, bundle, track, name = args
    if not bundle.endswith(".aab") or not os.path.isfile(bundle):
        raise SystemExit(f"no App Bundle at {bundle!r}")
    status = os.environ.get("PLAY_RELEASE_STATUS") or "completed"
    if status not in STATUSES:
        raise SystemExit(f"PLAY_RELEASE_STATUS must be one of {', '.join(STATUSES)}, got {status!r}")
    raw = os.environ.get("PLAY_SERVICE_ACCOUNT_JSON")
    if not raw:
        raise SystemExit("PLAY_SERVICE_ACCOUNT_JSON is not set")
    account = json.loads(raw)
    code = upload(package, bundle, track, name, status, access_token(account))
    print(f"{package}: version code {code} on track {track} ({status}), release {name}")


if __name__ == "__main__":
    main(sys.argv[1:])
