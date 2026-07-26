#!/usr/bin/env bash
set -euo pipefail

aar="${1:-build/ppvpn-core.aar}"
if [[ ! -f "${aar}" ]]; then
  echo "AAR not found: ${aar}" >&2
  exit 1
fi

unzip -tq "${aar}" >/dev/null
contents="$(unzip -Z1 "${aar}")"

for abi in armeabi-v7a arm64-v8a x86 x86_64; do
  if ! grep -Fxq "jni/${abi}/libgojni.so" <<<"${contents}"; then
    echo "AAR is missing jni/${abi}/libgojni.so" >&2
    exit 1
  fi
done

grep -Fxq "classes.jar" <<<"${contents}" ||
  { echo "AAR is missing classes.jar" >&2; exit 1; }
