#!/usr/bin/env python3
"""Installs App Store provisioning profiles by name from the App Store Connect API.

release.yml's ios job (#470), with the release environment's API key, so a
renewed profile needs no secret change:

    asc_profiles.py --serial <certificate serial> <name>=<bundle id>...

The key from the environment: ASC_KEY_ID, ASC_ISSUER_ID (a team key) and
ASC_KEY_P8 (the .p8 file's text). A name must match at most one active App
Store profile. Without one, the profile is created for that bundle ID on the
Apple Distribution certificate with the given serial (the one the job signs
with), which is the only change it makes in the account: the bundle IDs and
the certificate are made by hand. Each profile is installed where Xcode looks
for profiles, under its UUID, and its name, UUID and expiry are printed. Only
the standard library and openssl (for the ES256 signature) are used.
"""

import base64
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import time
import urllib.error
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


def call(bearer, path, query=None, body=None):
    url = f"{API}{path}" + (f"?{urllib.parse.urlencode(query)}" if query else "")
    headers = {"Authorization": f"Bearer {bearer}"}
    data = None
    if body is not None:
        headers["Content-Type"] = "application/json"
        data = json.dumps(body).encode()
    request = urllib.request.Request(url, data=data, headers=headers, method="POST" if body is not None else "GET")
    try:
        with urllib.request.urlopen(request, timeout=60) as response:
            return json.load(response)["data"]
    except urllib.error.HTTPError as error:
        raise SystemExit(f"{request.method} {path}: HTTP {error.code}: {error.read().decode(errors='replace')[:2000]}")


def find_profile(name, bearer):
    """The active App Store profile with exactly this name, or None."""
    data = call(bearer, "/v1/profiles", {
        "filter[name]": name,
        "filter[profileState]": "ACTIVE",
        "filter[profileType]": "IOS_APP_STORE",
    })
    # The filter matches substrings too.
    exact = [item for item in data if item["attributes"]["name"] == name]
    if len(exact) > 1:
        raise SystemExit(f"{len(exact)} active App Store profiles named {name!r}, need at most 1")
    return exact[0]["attributes"] if exact else None


def find_one(bearer, path, query, keep, what):
    found = [item for item in call(bearer, path, query) if keep(item["attributes"])]
    if len(found) != 1:
        raise SystemExit(f"{len(found)} {what}, need exactly 1")
    return found[0]["id"]


def create_profile(name, bundle, serial, bearer):
    # The identifier filter matches by prefix: com.peakpassvpn.ppvpn finds .tunnel too.
    bundle_id = find_one(bearer, "/v1/bundleIds", {"filter[identifier]": bundle},
                         lambda a: a["identifier"] == bundle and a["platform"] in ("IOS", "UNIVERSAL"),
                         f"iOS bundle IDs {bundle}")
    certificate_id = find_one(bearer, "/v1/certificates", {"filter[certificateType]": "DISTRIBUTION"},
                              lambda a: a["serialNumber"].lstrip("0").upper() == serial.lstrip("0").upper(),
                              f"Apple Distribution certificates with serial {serial}")
    return call(bearer, "/v1/profiles", body={"data": {
        "type": "profiles",
        "attributes": {"name": name, "profileType": "IOS_APP_STORE"},
        "relationships": {
            "bundleId": {"data": {"type": "bundleIds", "id": bundle_id}},
            "certificates": {"data": [{"type": "certificates", "id": certificate_id}]},
        },
    }})["attributes"]


def install(attributes, dirs=PROFILE_DIRS):
    content = base64.b64decode(attributes["profileContent"])
    for directory in dirs:
        directory.mkdir(parents=True, exist_ok=True)
        (directory / f"{attributes['uuid']}.mobileprovision").write_bytes(content)


def main(args):
    if len(args) < 3 or args[0] != "--serial" or not all("=" in arg for arg in args[2:]):
        raise SystemExit("usage: asc_profiles.py --serial <certificate serial> <name>=<bundle id>...")
    serial = args[1]
    bearer = token(os.environ["ASC_KEY_ID"], os.environ["ASC_ISSUER_ID"], os.environ["ASC_KEY_P8"])
    for arg in args[2:]:
        name, bundle = arg.split("=", 1)
        attributes = find_profile(name, bearer)
        made = attributes is None
        if made:
            attributes = create_profile(name, bundle, serial, bearer)
        install(attributes)
        print(f"{name}: {'created, ' if made else ''}{attributes['uuid']}, expires {attributes.get('expirationDate', '?')}")


if __name__ == "__main__":
    main(sys.argv[1:])
