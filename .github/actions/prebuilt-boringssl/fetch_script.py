"""Fetch one fixed script object, checking both Git blob and SHA256."""
import base64
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import urllib.error
import urllib.request

BLOB = "258aaa484798298cf4e59ca0b4757c79cc302786"
SHA256 = "dc6af85368d3cc6bf40c2b7ea9e025264b99adb4b3aa81cb39c8e36441802bda"
URL = "https://raw.githubusercontent.com/peakpassvpn/btls/16f0e320995602930eb11edc2be8d56b7145713f/scripts/prebuilt-boringssl.sh"

def verify(raw):
    if len(raw) != 2549 or hashlib.sha256(raw).hexdigest() != SHA256:
        raise ValueError("pinned prebuilt script SHA256 mismatch")
    if hashlib.sha1(b"blob " + str(len(raw)).encode() + b"\0" + raw).hexdigest() != BLOB:
        raise ValueError("pinned prebuilt script Git blob mismatch")

def fetch(output):
    transport = "raw"
    try:
        with urllib.request.urlopen(URL, timeout=30) as response:
            raw = response.read()
    except (urllib.error.URLError, TimeoutError):
        # Alternate transport for the SAME immutable public object. No version fallback.
        payload = json.loads(subprocess.check_output(["gh", "api", "repos/peakpassvpn/btls/git/blobs/" + BLOB]))
        if payload["sha"] != BLOB or payload["encoding"] != "base64":
            raise ValueError("unexpected pinned script blob response")
        raw = base64.b64decode(payload["content"])
        transport = "git-blob-api"
    # A successful response with wrong bytes is fatal, never a fallback.
    verify(raw)
    Path(output).write_bytes(raw)
    print("Verified fixed prebuilt script", SHA256, transport)

if __name__ == "__main__":
    fetch(sys.argv[1])
