#!/usr/bin/env python3
"""Installs App Store provisioning profiles by name from the App Store Connect API.

release.yml's ios job (#470): the profiles are made in the account once, and
the build downloads them each time with the release environment's API key,
so a renewed profile needs no secret change.

    asc_profiles.py <profile name>...

The key from the environment: ASC_KEY_ID, ASC_ISSUER_ID (a team key) and
ASC_KEY_P8 (the .p8 file's text). Each name must match exactly one active
App Store profile; it is installed where Xcode looks for profiles, under its
UUID, and its name, UUID and expiry are printed. Only the standard library
and openssl (for the ES256 signature) are used.
"""

import base64
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import time
import urllib.parse
import urllib.request

API = os.environ.get("ASC_API_BASE", "https://api.appstoreconnect.apple.com")
# Xcode 16 reads the first; older Xcodes the second.
PROFILE_DIRS = [
    pathlib.Path.home() / "Library/Developer/Xcode/UserData/Provisioning Profiles",
    pathlib.Path.home() / "Library/MobileDevice/Provisioning Profiles",
]


def b64url(data):
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode()


def der_to_raw(der, size=32):
    """An ECDSA signature from DER (what openssl prints) to r || s (what JWS wants)."""
    if len(der) < 8 or der[0] != 0x30:
        raise ValueError("not a DER ECDSA signature")
    pos = 2 if der[1] < 0x80 else 2 + (der[1] & 0x7F)
    parts = []
    for _ in range(2):
        if der[pos] != 0x02:
            raise ValueError("not a DER ECDSA signature")
        length = der[pos + 1]
        value = der[pos + 2 : pos + 2 + length].lstrip(b"\0")
        if len(value) > size:
            raise ValueError("signature integer too long")
        parts.append(value.rjust(size, b"\0"))
        pos += 2 + length
    return parts[0] + parts[1]


def token(key_id, issuer, p8, now=None):
    """A 10-minute App Store Connect API token for a team key."""
    now = int(time.time() if now is None else now)
    header = {"alg": "ES256", "kid": key_id, "typ": "JWT"}
    payload = {"iss": issuer, "iat": now, "exp": now + 600, "aud": "appstoreconnect-v1"}
    signing_input = f"{b64url(json.dumps(header).encode())}.{b64url(json.dumps(payload).encode())}"
    # Created 0600.
    with tempfile.NamedTemporaryFile("w", suffix=".p8") as key:
        key.write(p8)
        key.flush()
        der = subprocess.run(
            ["openssl", "dgst", "-sha256", "-sign", key.name],
            input=signing_input.encode(), capture_output=True, check=True,
        ).stdout
    return f"{signing_input}.{b64url(der_to_raw(der))}"


def find_profile(name, bearer):
    query = urllib.parse.urlencode({
        "filter[name]": name,
        "filter[profileState]": "ACTIVE",
        "filter[profileType]": "IOS_APP_STORE",
    })
    request = urllib.request.Request(f"{API}/v1/profiles?{query}", headers={"Authorization": f"Bearer {bearer}"})
    with urllib.request.urlopen(request, timeout=60) as response:
        data = json.load(response)["data"]
    # The filter matches substrings too.
    exact = [item for item in data if item["attributes"]["name"] == name]
    if len(exact) != 1:
        raise SystemExit(f"{len(exact)} active App Store profiles named {name!r}, need exactly 1")
    return exact[0]["attributes"]


def install(attributes, dirs=PROFILE_DIRS):
    content = base64.b64decode(attributes["profileContent"])
    for directory in dirs:
        directory.mkdir(parents=True, exist_ok=True)
        (directory / f"{attributes['uuid']}.mobileprovision").write_bytes(content)


def main(names):
    if not names:
        raise SystemExit("usage: asc_profiles.py <profile name>...")
    bearer = token(os.environ["ASC_KEY_ID"], os.environ["ASC_ISSUER_ID"], os.environ["ASC_KEY_P8"])
    for name in names:
        attributes = find_profile(name, bearer)
        install(attributes)
        print(f"{name}: {attributes['uuid']}, expires {attributes.get('expirationDate', '?')}")


if __name__ == "__main__":
    main(sys.argv[1:])
