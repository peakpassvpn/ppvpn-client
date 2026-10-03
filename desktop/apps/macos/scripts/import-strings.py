#!/usr/bin/env python3
"""Generate the macOS string catalogs from the shared strings.json.

    apps/macos/scripts/import-strings.py [path/to/strings.json]

The source of truth is apps/shared/PPVPN.App.Core/Strings/strings.json
({"zh": {key: text}, "en": {key: text}}), shared with Windows and Linux:

  * Localizable.xcstrings gets every non-`Error_` key unchanged as its key
    (placeholders like {n} stay; `tr(_:_:)` fills them at runtime). Keys
    prefixed `x_` are macOS-only and are kept; other keys not in
    strings.json are dropped.
  * Errors.xcstrings gets every `Error_<Variant>` as `<Variant>`, the key
    `ErrorCode.message` looks up.
"""
import json
import sys
from pathlib import Path

APP = Path(__file__).resolve().parent.parent
REPO = APP.parent.parent
DEFAULT_SOURCE = REPO / "apps/shared/PPVPN.App.Core/Strings/strings.json"
RESOURCES = APP / "PPVPN/Resources"


def unit(value):
    return {"stringUnit": {"state": "translated", "value": value}}


def entry(zh, en):
    return {"extractionState": "manual", "localizations": {"zh-Hans": unit(zh), "en": unit(en)}}


def write(path, strings):
    out = {"sourceLanguage": "zh-Hans", "strings": dict(sorted(strings.items())), "version": "1.0"}
    path.write_text(json.dumps(out, ensure_ascii=False, indent=2) + "\n")


def main():
    source = Path(sys.argv[1]) if len(sys.argv) > 1 else DEFAULT_SOURCE
    shared = json.loads(source.read_text())
    zh, en = shared["zh"], shared["en"]
    mismatch = sorted(set(zh) ^ set(en))
    if mismatch:
        sys.exit(f"zh/en key mismatch: {mismatch}")

    localizable_path = RESOURCES / "Localizable.xcstrings"
    existing = json.loads(localizable_path.read_text()).get("strings", {}) if localizable_path.exists() else {}
    local = {key: value for key, value in existing.items() if key.startswith("x_")}

    localizable = {key: entry(zh[key], en[key]) for key in zh if not key.startswith("Error_")}
    localizable.update(local)
    errors = {key[len("Error_"):]: entry(zh[key], en[key]) for key in zh if key.startswith("Error_")}

    write(localizable_path, localizable)
    write(RESOURCES / "Errors.xcstrings", errors)
    print(f"{len(localizable) - len(local)} shared + {len(local)} x_ keys -> Localizable.xcstrings; "
          f"{len(errors)} -> Errors.xcstrings")


if __name__ == "__main__":
    main()
