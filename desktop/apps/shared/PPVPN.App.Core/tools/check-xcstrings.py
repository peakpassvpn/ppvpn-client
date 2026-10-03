#!/usr/bin/env python3
"""Check the macOS string catalogs against the shared strings.json.

The Windows and Linux apps read apps/shared/PPVPN.App.Core/Strings/strings.json
(the design handoff's keys plus Error_*). The macOS catalogs are generated from
it by apps/macos/scripts/import-strings.py, keyed by the same strings.json keys.
This script reports where they drift apart:

  * Localizable.xcstrings: each strings.json key must be a catalog key whose
    zh-Hans and en values match. (Catalogs keyed by the Chinese source text,
    the older macOS layout, are still matched through that text.)
  * Errors.xcstrings: keyed by ErrorCode variant name; zh-Hans and en values must
    match Error_<variant>.

Placeholders are compared loosely: {name}, %@, %1$@, %lld, %u and %d all count
as one placeholder. Keys for other platforms (revealWin, uac*, polkit*, …) and
keys macOS does not use are skipped with --skip or listed in SKIP below.

Usage:
  apps/shared/PPVPN.App.Core/tools/check-xcstrings.py [--strings PATH] [--macos DIR]
      [--only KEY,KEY] [--skip KEY,KEY] [--strict]

Exit status: 0 when everything checked matches (or without --strict), 1 otherwise.
"""
import argparse
import json
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[3]
DEFAULT_STRINGS = HERE.parent / "Strings" / "strings.json"
DEFAULT_MACOS = REPO / "apps" / "macos" / "PPVPN" / "Resources"

# Windows / Linux / prototype-only keys the macOS app has no reason to carry.
SKIP = {
    "revealWin", "revealLinux", "settingsMenuWin", "winLaunchApprove", "openWinStartup",
    "stillRunning", "stillRunningD", "hiddenIcons", "uacQ", "uacPub", "uacYes", "uacNo",
    "polkitT", "polkitD", "polkitDu", "polkitBtn", "mainMenu", "preferences", "keyboard",
    "authMacT", "authMacD", "authMacOk", "authMacUn", "authUser", "authPass",
    "h_off", "d_off", "total", "live", "turnOn", "turnOff", "standardMode", "status",
    "colProxy", "collapse", "close", "now", "proxyOffNote", "tier_pro", "tier_opt", "apiPh",
}

PLACEHOLDER = re.compile(r"\{\w+\}|%(?:\d+\$)?(?:@|lld|ld|llu|lu|u|d|f|s)")


def normalize(text: str) -> str:
    """One canonical form for comparing texts across formats."""
    return PLACEHOLDER.sub("%@", text).strip()


def xc_value(entry: dict, language: str):
    unit = entry.get("localizations", {}).get(language, {}).get("stringUnit")
    return unit.get("value") if unit else None


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--strings", type=Path, default=DEFAULT_STRINGS, help="shared strings.json")
    parser.add_argument("--macos", type=Path, default=DEFAULT_MACOS, help="directory with the .xcstrings files")
    parser.add_argument("--only", default="", help="comma-separated keys to check (default: all)")
    parser.add_argument("--skip", default="", help="comma-separated keys to skip in addition to the built-in list")
    parser.add_argument("--strict", action="store_true", help="exit 1 when anything differs")
    args = parser.parse_args()

    shared = json.loads(args.strings.read_text(encoding="utf-8"))
    zh, en = shared["zh"], shared["en"]
    only = {k for k in args.only.split(",") if k}
    skip = SKIP | {k for k in args.skip.split(",") if k}

    localizable = json.loads((args.macos / "Localizable.xcstrings").read_text(encoding="utf-8"))["strings"]
    errors = json.loads((args.macos / "Errors.xcstrings").read_text(encoding="utf-8"))["strings"]
    by_source = {normalize(key): (key, entry) for key, entry in localizable.items()}

    missing, english, chinese, error_diffs = [], [], [], []
    for key, zh_text in zh.items():
        if only and key not in only:
            continue
        if key.startswith("Error_"):
            variant = key[len("Error_"):]
            entry = errors.get(variant)
            if entry is None:
                if variant not in ("NotSignedIn", "StandardNotReady", "NotImplemented", "Unexpected", "Unknown"):
                    error_diffs.append(f"{key}: missing from Errors.xcstrings")
                continue
            for lang, text in (("zh-Hans", zh_text), ("en", en[key])):
                value = xc_value(entry, lang)
                if value is not None and normalize(value) != normalize(text):
                    error_diffs.append(f"{key} [{lang}]: macOS “{value}” ≠ shared “{text}”")
            continue
        if key in skip:
            continue
        if key in localizable:
            source, entry = key, localizable[key]
            value = xc_value(entry, "zh-Hans")
            if value is not None and normalize(value) != normalize(zh_text):
                chinese.append(f"{key}: macOS “{value}” ≠ shared “{zh_text}”")
        else:
            found = by_source.get(normalize(zh_text))
            if found is None:
                missing.append(f"{key}: “{zh_text}”")
                continue
            source, entry = found
        value = xc_value(entry, "en")
        if value is not None and normalize(value) != normalize(en[key]):
            english.append(f"{key}: macOS “{value}” ≠ shared “{en[key]}” (source “{source}”)")

    for title, rows in (("Not in Localizable.xcstrings (by key or Chinese text)", missing),
                        ("Chinese differs", chinese),
                        ("English differs", english),
                        ("Errors.xcstrings differs", error_diffs)):
        print(f"{title}: {len(rows)}")
        for row in rows:
            print(f"  {row}")
    drift = bool(missing or chinese or english or error_diffs)
    return 1 if drift and args.strict else 0


if __name__ == "__main__":
    sys.exit(main())
