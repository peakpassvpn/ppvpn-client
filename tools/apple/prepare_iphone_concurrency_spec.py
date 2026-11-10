#!/usr/bin/env python3
"""Prepare private staging files only. Never launches, copies to device or opens a socket."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import time
import uuid


def prepare(draft_path, manifest_path, destination):
    draft = json.loads(Path(draft_path).read_text())
    manifest_bytes = Path(manifest_path).read_bytes()
    manifest = json.loads(manifest_bytes)
    now = time.time()
    if len(manifest_bytes) > 16384 or manifest.get("schema") != 1:
        raise ValueError("invalid manifest schema or size")
    expiry = min(now + 240, float(manifest["expires_unix"]))
    if expiry - now < 30:
        raise ValueError("manifest needs at least30s remaining")
    for key in ["min_available_bytes", "max_footprint_bytes", "duration_seconds"]:
        if not isinstance(draft.get(key), int) or draft[key] <= 0:
            raise ValueError("explicit positive " + key + " required")
    if any(key in draft for key in ["run_id", "run_token", "expires_unix", "target_manifest_sha256"]):
        raise ValueError("draft must not reuse run identity/capability")
    run_id = str(uuid.uuid4())
    token = str(uuid.uuid4())
    digest = hashlib.sha256(manifest_bytes).hexdigest()
    request = dict(draft, schema=1, run_id=run_id, run_token=token,
                   expires_unix=expiry, target_manifest_sha256=digest)
    request_bytes = (json.dumps(request, indent=2, sort_keys=True) + "\n").encode()
    if len(request_bytes) > 262144:
        raise ValueError("request too large")
    output = Path(destination)
    output.mkdir(mode=0o700, parents=True, exist_ok=False)
    os.chmod(output, 0o700)
    files = {
        "request.json": request_bytes,
        "approved-targets.json": manifest_bytes,
        "fixture-authorization.json": (json.dumps({"tokens": [token]}) + "\n").encode(),
        "public-receipt.json": (json.dumps({"run_id": run_id, "target_manifest_sha256": digest,
            "request_sha256": hashlib.sha256(request_bytes).hexdigest(),
            "note": "staging_only_native_validation_and_route_proof_still_required"}, indent=2) + "\n").encode(),
    }
    for name, data in files.items():
        path = output / name
        with path.open("xb") as stream:
            os.chmod(path, 0o600)
            stream.write(data)
    return {"run_id": run_id, "target_manifest_sha256": digest, "staging_directory": str(output)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--draft", required=True)
    parser.add_argument("--manifest", required=True)
    parser.add_argument("--out", required=True)
    args = parser.parse_args()
    print(json.dumps(prepare(args.draft, args.manifest, args.out), sort_keys=True))


if __name__ == "__main__":
    main()
