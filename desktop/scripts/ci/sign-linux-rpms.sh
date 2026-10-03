#!/usr/bin/env bash
# Signs, in place, the rpms of one release before they become release assets, and records the
# signed app package in its release metadata. A signature changes the file, so this runs
# before anything (the release's checksums, the update site, the dnf repository) reads the
# metadata: the dnf repository refers to the release asset and no longer hosts a copy.
#
# Usage: scripts/ci/sign-linux-rpms.sh <dir>
#
#   <dir>/release-meta-linux-x64-rpm.json   names the app's rpm; length and sha256 are rewritten
#   <dir>/<the rpm it names>                signed
#   <dir>/ppvpn-release-*.noarch.rpm        signed when present (the repository setup package)
#
# An unsigned app rpm must be the one its metadata describes (length, sha256). An rpm already
# signed by this key is left as it is (a signature carries a timestamp) and only checked, so a
# second run changes nothing; one signed by any other key is refused.
#
# Environment:
#   PKG_GPG_PRIVATE_KEY   ASCII-armored secret signing key (imported into a throwaway
#                         GNUPGHOME removed on exit; never printed)
#   PKG_GPG_PASSPHRASE    its passphrase, if any
#   PKG_GPG_FINGERPRINT   the key expected in PKG_GPG_PRIVATE_KEY (required: the production
#                         fingerprint, or a test key's)
#
# Tools: gpg, rpm, rpmsign, python3.
set -euo pipefail

die() { echo "sign-linux-rpms: $*" >&2; exit 1; }

[[ $# -eq 1 ]] || die "usage: sign-linux-rpms.sh <dir>"
DIR="$1"
: "${PKG_GPG_PRIVATE_KEY:?PKG_GPG_PRIVATE_KEY is required}"
: "${PKG_GPG_FINGERPRINT:?PKG_GPG_FINGERPRINT is required}"
FINGERPRINT="$(printf '%s' "$PKG_GPG_FINGERPRINT" | tr -d ' ' | tr 'a-f' 'A-F')"
[[ "$FINGERPRINT" =~ ^[0-9A-F]{40}$ ]] || die "PKG_GPG_FINGERPRINT is not a 40-digit fingerprint"
[[ -d "$DIR" ]] || die "$DIR is not a directory"
for tool in gpg gpgconf rpm rpmsign python3; do
  command -v "$tool" >/dev/null 2>&1 || die "$tool is required"
done
META="$DIR/release-meta-linux-x64-rpm.json"
[[ -f "$META" ]] || die "missing $META"

WORK="$(mktemp -d)"
export GNUPGHOME="$WORK/gnupg"
cleanup() {
  gpgconf --kill all >/dev/null 2>&1 || true
  rm -rf "$WORK"
}
trap cleanup EXIT
mkdir -m 700 "$GNUPGHOME"

# --- the release metadata -----------------------------------------------------------------
#   meta_tool file <meta>            prints the rpm's file name, after checking the metadata
#   meta_tool matches <meta> <rpm>   fails unless the rpm has the metadata's length and sha256
#   meta_tool update <meta> <rpm>    rewrites length and sha256 from the rpm
meta_tool() {
  python3 - "$@" <<'PY'
import hashlib, json, os, sys

def refuse(message):
    sys.exit(f"sign-linux-rpms: {message}")

command, path = sys.argv[1], sys.argv[2]
with open(path, encoding="utf-8") as f:
    text = f.read()
try:
    meta = json.loads(text)
except ValueError as error:
    refuse(f"{path}: not JSON ({error})")
if not isinstance(meta, dict) or meta.get("schema") != 1:
    refuse(f"{path}: unsupported release-meta schema")
if meta.get("platform") != "linux-x64-rpm":
    refuse(f"{path}: metadata is for {meta.get('platform')!r}, not linux-x64-rpm")
name = meta.get("file")
if not isinstance(name, str) or not name.endswith(".rpm") or os.path.basename(name) != name:
    refuse(f"{path}: file is not a bare .rpm file name")
if command == "file":
    print(name)
    sys.exit(0)

rpm = sys.argv[3]
digest = hashlib.sha256()
with open(rpm, "rb") as f:
    for chunk in iter(lambda: f.read(1 << 20), b""):
        digest.update(chunk)
length, sha256 = os.path.getsize(rpm), digest.hexdigest()
if command == "matches":
    if meta.get("length") != length or meta.get("sha256") != sha256:
        refuse(f"{name} is not the package {os.path.basename(path)} describes (length, sha256)")
elif command == "update":
    meta["length"], meta["sha256"] = length, sha256
    out = json.dumps(meta, indent=2) + ("\n" if text.endswith("\n") else "")
    with open(path + ".part", "w", encoding="utf-8") as f:
        f.write(out)
    os.replace(path + ".part", path)
else:
    refuse(f"unknown command {command}")
PY
}

RPM_FILE="$(meta_tool file "$META")"
RPM="$DIR/$RPM_FILE"
[[ -f "$RPM" ]] || die "$META: $RPM_FILE not found"

# --- the signing key ----------------------------------------------------------------------
printf '%s\n' "$PKG_GPG_PRIVATE_KEY" | gpg --batch --quiet --import 2>/dev/null \
  || die "could not import PKG_GPG_PRIVATE_KEY"
SECRET_KEYS="$(gpg --batch --list-secret-keys --with-colons | awk -F: '$1=="fpr"{print $10}')"
grep -qx "$FINGERPRINT" <<<"$SECRET_KEYS" || die "the signing key is not $FINGERPRINT"
RPM_SIGN_EXTRA=""
if [[ -n "${PKG_GPG_PASSPHRASE:-}" ]]; then
  printf '%s' "$PKG_GPG_PASSPHRASE" >"$GNUPGHOME/passphrase"
  chmod 600 "$GNUPGHOME/passphrase"
  RPM_SIGN_EXTRA="--pinentry-mode loopback --passphrase-file $GNUPGHOME/passphrase"
fi
gpg --batch --armor --export "$FINGERPRINT" >"$WORK/ppvpn.asc"
# rpm names the key that made a signature by its 64-bit ID: the primary key's or a subkey's.
KEY_IDS="$(gpg --batch --list-keys --with-colons "$FINGERPRINT" \
  | awk -F: '$1=="pub"||$1=="sub"{print tolower($5)}')"
[[ -n "$KEY_IDS" ]] || die "could not list the key IDs of $FINGERPRINT"
RPMDB="$WORK/rpmdb"
mkdir -p "$RPMDB"
rpm --dbpath "$RPMDB" --import "$WORK/ppvpn.asc"

# unsigned | ours; refuses a signature of any other key, or one it cannot attribute.
signature_state() {
  local signatures line id state=unsigned
  signatures="$(rpm --dbpath "$RPMDB" -qp \
    --qf '%{SIGPGP:pgpsig}\n%{SIGGPG:pgpsig}\n%{RSAHEADER:pgpsig}\n%{DSAHEADER:pgpsig}\n' "$1")" \
    || die "$1: could not read the package"
  while IFS= read -r line; do
    [[ -n "$line" && "$line" != "(none)" ]] || continue
    if [[ "$line" =~ Key\ ID\ ([0-9A-Fa-f]{16}) ]]; then
      id="$(printf '%s' "${BASH_REMATCH[1]}" | tr 'A-F' 'a-f')"
      grep -qx "$id" <<<"$KEY_IDS" || die "$1 is already signed by another key ($id)"
      state=ours
    else
      die "$1 carries a signature whose key could not be read"
    fi
  done <<<"$signatures"
  echo "$state"
}
check_rpm() {
  local verdict
  verdict="$(rpm --dbpath "$RPMDB" -K "$1")" || die "$1: signature check failed"
  grep -q 'digests signatures OK' <<<"$verdict" || die "$1: signature check failed"
}
sign_rpm() {
  # %__gpg differs between distributions (Ubuntu's rpm expects gpg2): name ours.
  rpmsign --define "__gpg $(command -v gpg)" --define "_gpg_name $FINGERPRINT" --define "_gpg_path $GNUPGHOME" \
    ${RPM_SIGN_EXTRA:+--define "_gpg_sign_cmd_extra_args $RPM_SIGN_EXTRA"} --addsign "$1" >/dev/null
}

# --- the app's rpm ------------------------------------------------------------------------
state="$(signature_state "$RPM")"
if [[ "$state" == unsigned ]]; then
  meta_tool matches "$META" "$RPM"
  sign_rpm "$RPM"
  [[ "$(signature_state "$RPM")" == ours ]] || die "$RPM: rpmsign left no signature"
  echo "signed $RPM_FILE"
else
  echo "$RPM_FILE is already signed by $FINGERPRINT"
fi
check_rpm "$RPM"
meta_tool update "$META" "$RPM"
meta_tool matches "$META" "$RPM"

# --- the repository setup package ---------------------------------------------------------
while IFS= read -r setup; do
  [[ -n "$setup" ]] || continue
  state="$(signature_state "$setup")"
  if [[ "$state" == unsigned ]]; then
    sign_rpm "$setup"
    [[ "$(signature_state "$setup")" == ours ]] || die "$setup: rpmsign left no signature"
    echo "signed $(basename "$setup")"
  else
    echo "$(basename "$setup") is already signed by $FINGERPRINT"
  fi
  check_rpm "$setup"
done < <(find "$DIR" -maxdepth 1 -type f -name 'ppvpn-release-*.noarch.rpm' | sort)

echo "$(basename "$META"): length and sha256 are the signed $RPM_FILE's"
