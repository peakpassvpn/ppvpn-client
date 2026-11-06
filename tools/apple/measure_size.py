#!/usr/bin/env python3
"""Read-only Apple artifact inventory and like-for-like size/ABI comparison."""
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import plistlib
import stat
import struct
import subprocess
import zipfile

CPUS = {7: "i386", 0x01000007: "x86_64", 12: "arm", 0x0100000C: "arm64"}
REQUIRED_CONDITIONS = {"architecture", "optimization", "strip", "signing", "toolchain", "build_number"}
PREFIXES = ("frb_", "store_dart", "uniffi_", "ffi_", "ppvpn_tunnel_")


def macho(data):
    magic = data[:4]
    thin = {b"\xce\xfa\xed\xfe": "<", b"\xcf\xfa\xed\xfe": "<",
            b"\xfe\xed\xfa\xce": ">", b"\xfe\xed\xfa\xcf": ">"}
    fat = {b"\xca\xfe\xba\xbe": (">", 20), b"\xbe\xba\xfe\xca": ("<", 20),
           b"\xca\xfe\xba\xbf": (">", 32), b"\xbf\xba\xfe\xca": ("<", 32)}
    if magic in thin and len(data) >= 16:
        cpu, _, filetype = struct.unpack_from(thin[magic] + "III", data, 4)
        return {"architectures": [CPUS.get(cpu, hex(cpu))], "filetype": filetype}
    if magic in fat and len(data) >= 8:
        endian, stride = fat[magic]
        count, = struct.unpack_from(endian + "I", data, 4)
        if count > 64 or len(data) < 8 + count * stride:
            raise ValueError("invalid or oversized universal Mach-O header")
        cpus = [struct.unpack_from(endian + "I", data, 8 + i * stride)[0] for i in range(count)]
        return {"architectures": sorted({CPUS.get(cpu, hex(cpu)) for cpu in cpus}),
                "filetype": "universal"}
    return None


def owner(name):
    parts = PurePosixPath(name).parts
    for part in parts:
        if part.endswith(".appex"):
            return "extension:" + part
    if "LoginItems" in parts or "XPCServices" in parts:
        return "helper"
    return "main_app"


def file_row(name, stream):
    digest = hashlib.sha256()
    head = stream.read(4096)
    digest.update(head)
    size = len(head)
    plist_parts = [head] if PurePosixPath(name).name == "Info.plist" else None
    for chunk in iter(lambda: stream.read(1024 * 1024), b""):
        digest.update(chunk)
        size += len(chunk)
        if plist_parts is not None:
            if size <= 1024 * 1024:
                plist_parts.append(chunk)
            else:
                plist_parts = None
    binary = macho(head)
    category = "resource"
    if binary:
        category = "shared_library" if (binary["filetype"] == 6 or ".framework/" in name
                                        or name.endswith(".dylib")) else "program"
    bundle = {}
    if plist_parts is not None:
        try:
            info = plistlib.loads(b"".join(plist_parts))
            bundle = {k: info[k] for k in ("CFBundleIdentifier", "CFBundleExecutable",
                "CFBundleVersion", "CFBundleShortVersionString", "MinimumOSVersion",
                "LSMinimumSystemVersion", "DTPlatformName") if k in info}
        except (ValueError, plistlib.InvalidFileException):
            pass
    return {"path": name, "bytes": size, "sha256": digest.hexdigest(),
            **({"bundle": bundle} if bundle else {}),
            "owner": owner(name), "category": category, **({"macho": binary} if binary else {})}


def directory_rows(root):
    rows, links = [], []
    for parent, directories, files in os.walk(root, followlinks=False):
        directories.sort()
        for name in directories[:]:
            path = Path(parent) / name
            if path.is_symlink():
                links.append(path.relative_to(root).as_posix())
                directories.remove(name)
        for name in sorted(files):
            path = Path(parent) / name
            relative = path.relative_to(root).as_posix()
            if path.is_symlink():
                links.append(relative)
            elif path.is_file():
                with path.open("rb") as stream:
                    rows.append(file_row(relative, stream))
    return rows, sorted(links)


def summarize(rows, links):
    components = {}
    architectures = set()
    for row in rows:
        key = row["owner"] + "/" + row["category"]
        components[key] = components.get(key, 0) + row["bytes"]
        architectures.update(row.get("macho", {}).get("architectures", []))
    return {"regular_file_bytes": sum(row["bytes"] for row in rows), "file_count": len(rows),
            "architectures": sorted(architectures), "components": components,
            "symlinks_not_dereferenced": links, "files": sorted(rows, key=lambda r: r["path"])}


def blob(path):
    with path.open("rb") as stream:
        row = file_row(path.name, stream)
    return {key: row[key] for key in ("bytes", "sha256")}


def inventory(args):
    path = args.artifact
    conditions = json.loads(args.conditions.read_text())
    removals = sorted(getattr(args, "approved_removal", []))
    if args.platform == "macos" and removals:
        raise ValueError("mobile feature removals cannot be applied to macOS")
    required = REQUIRED_CONDITIONS
    if not required <= conditions.keys():
        raise ValueError("conditions missing: " + ", ".join(sorted(required - conditions.keys())))
    result = {"schema": 1, "platform": args.platform, "kind": args.kind,
              "source_commit": args.source_commit, "functional_baseline": args.functional_baseline,
              "conditions": conditions, "artifact_name": path.name, "approved_removals": removals,
              "provenance": "Source/conditions supplied by caller; preserve build logs separately.",
              "runtime_verdict": "Not assessed by a file-size inventory."}
    if args.kind in ("app", "xcarchive"):
        if not path.is_dir():
            raise ValueError("expected an artifact directory")
        rows, links = directory_rows(path)
        result["artifact"] = summarize(rows, links)
        if args.kind == "xcarchive":
            apps = list((path / "Products/Applications").glob("*.app"))
            if len(apps) != 1:
                raise ValueError("archive must have exactly one application")
            result["application"] = summarize(*directory_rows(apps[0]))
        else:
            result["application"] = result["artifact"]
    elif args.kind == "ipa":
        rows, links = [], []
        with zipfile.ZipFile(path) as archive:
            names = archive.namelist()
            if len(set(names)) != len(names):
                raise ValueError("duplicate ZIP members")
            for info in archive.infolist():
                name = PurePosixPath(info.filename)
                if name.is_absolute() or ".." in name.parts:
                    raise ValueError("unsafe ZIP member")
                if info.is_dir():
                    continue
                if stat.S_ISLNK(info.external_attr >> 16):
                    links.append(info.filename)
                    continue
                with archive.open(info) as stream:
                    rows.append(file_row(info.filename, stream))
        roots = {PurePosixPath(r["path"]).parts[1] for r in rows
                 if r["path"].startswith("Payload/") and len(PurePosixPath(r["path"]).parts) > 2
                 and PurePosixPath(r["path"]).parts[1].endswith(".app")}
        if len(roots) != 1:
            raise ValueError("IPA must have exactly one Payload application")
        prefix = "Payload/" + next(iter(roots)) + "/"
        app_rows = [{**r, "path": r["path"][len(prefix):]} for r in rows if r["path"].startswith(prefix)]
        result["artifact"] = summarize(rows, links)
        result["application"] = summarize(app_rows, [p[len(prefix):] for p in links if p.startswith(prefix)])
        result["container"] = blob(path)
        result["device_download_size"] = None
        result["device_download_note"] = "IPA compressed bytes are not App Store device download size."
    elif args.kind in ("dmg", "pkg"):
        result["container"] = blob(path)
        result["application"] = None
        result["inspection_note"] = "Opaque installer: no mount, extraction or installation performed."
    return result


def compare(before, after):
    def incomplete(value):
        return (value is None
                or isinstance(value, dict) and any(incomplete(v) for v in value.values())
                or isinstance(value, list) and any(incomplete(v) for v in value))

    if any(not isinstance(row.get("conditions"), dict)
           or not REQUIRED_CONDITIONS <= row["conditions"].keys()
           or incomplete(row["conditions"]) for row in (before, after)):
        raise ValueError("incomplete build conditions: reproduce and attest both builds first")
    for key in ("schema", "platform", "kind", "functional_baseline", "conditions"):
        if before.get(key) != after.get(key):
            raise ValueError("incomparable measurements: " + key)
    result = {"baseline_commit": before["source_commit"], "candidate_commit": after["source_commit"],
              "runtime_verdict": "Requires independent one-active-core and functionality evidence.",
              "savings": {}, "baseline_approved_removals": before.get("approved_removals", []),
              "candidate_approved_removals": after.get("approved_removals", []),
              "feature_scope_note": "Approved mobile removals must be disclosed; savings are not solely runtime deduplication."}
    metrics = [("application", "regular_file_bytes"), ("container", "bytes")]
    if before["kind"] == "xcarchive":
        metrics.append(("artifact", "regular_file_bytes"))
    for field, metric in metrics:
        if before.get(field) is not None and after.get(field) is not None:
            delta = before[field][metric] - after[field][metric]
            result["savings"][field] = {"baseline_bytes": before[field][metric],
                "candidate_bytes": after[field][metric], "saved_bytes": delta,
                "saved_MB": delta / 1_000_000, "saved_MiB": delta / 1_048_576}
    if before.get("application") and after.get("application"):
        a, b = before["application"], after["application"]
        if a["architectures"] != b["architectures"]:
            raise ValueError("incomparable actual Mach-O architectures")
        result["component_savings_bytes"] = {
            key: a["components"].get(key, 0) - b["components"].get(key, 0)
            for key in sorted(a["components"].keys() | b["components"].keys())}
    return result


def exports(path):
    text = subprocess.check_output(["nm", "-gU", str(path)], text=True, stderr=subprocess.PIPE)
    return {line.split()[-1].removeprefix("_") for line in text.splitlines()
            if line.split() and line.split()[-1].startswith("_")}


def abi(args):
    a, b = exports(args.baseline_library), exports(args.candidate_library)
    expected = ({line.strip().removeprefix("_") for line in args.required_symbols.read_text().splitlines()
                 if line.strip() and not line.lstrip().startswith("#")} if args.required_symbols else
                {name for name in a if name.startswith(PREFIXES)})
    if not expected or expected - a:
        raise ValueError("empty symbol contract or baseline lacks required exports")
    missing = sorted(expected - b)
    result = {"required_count": len(expected), "missing": missing,
              "extra_api_exports": sorted({n for n in b if n.startswith(PREFIXES)} - expected),
              "compatible_exports": not missing,
              "note": "Export presence alone does not validate ABI types, checksums or IPC semantics."}
    if args.baseline_binding or args.candidate_binding:
        if not (args.baseline_binding and args.candidate_binding):
            raise ValueError("provide both generated binding files")
        result["generated_binding_identical"] = blob(args.baseline_binding) == blob(args.candidate_binding)
        result["compatible_exports"] &= result["generated_binding_identical"]
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    inv = commands.add_parser("inventory")
    inv.add_argument("--artifact", type=Path, required=True)
    inv.add_argument("--platform", choices=("macos", "ios"), required=True)
    inv.add_argument("--kind", choices=("app", "xcarchive", "ipa", "dmg", "pkg"), required=True)
    inv.add_argument("--source-commit", required=True)
    inv.add_argument("--functional-baseline", required=True)
    inv.add_argument("--conditions", type=Path, required=True)
    inv.add_argument("--approved-removal", action="append", default=[], choices=(
        "mobile-local-proxy-server", "mobile-proxy-settings-ui", "app-standard-forwarder"))
    cmp = commands.add_parser("compare")
    cmp.add_argument("baseline", type=Path)
    cmp.add_argument("candidate", type=Path)
    ab = commands.add_parser("abi")
    ab.add_argument("--baseline-library", type=Path, required=True)
    ab.add_argument("--candidate-library", type=Path, required=True)
    ab.add_argument("--required-symbols", type=Path)
    ab.add_argument("--baseline-binding", type=Path)
    ab.add_argument("--candidate-binding", type=Path)
    for cmd in (inv, cmp, ab):
        cmd.add_argument("--output", type=Path)
    args = parser.parse_args()
    try:
        result = (inventory(args) if args.command == "inventory" else
                  compare(json.loads(args.baseline.read_text()), json.loads(args.candidate.read_text()))
                  if args.command == "compare" else abi(args))
    except (ValueError, OSError, subprocess.CalledProcessError, zipfile.BadZipFile) as error:
        parser.error(str(error))
    data = json.dumps(result, indent=2, sort_keys=True) + "\n"
    if args.output:
        args.output.write_text(data)
    else:
        print(data, end="")
    return 1 if args.command == "abi" and not result["compatible_exports"] else 0


if __name__ == "__main__":
    raise SystemExit(main())
