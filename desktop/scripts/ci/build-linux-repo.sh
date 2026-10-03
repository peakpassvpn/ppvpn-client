#!/usr/bin/env bash
# Builds, from release assets alone, the signed apt and dnf repositories of one channel: the
# linux/ tree of the site, as clients of https://pkg.peakpassvpn.com read it.
#
#   linux/apt/pool/main/p/ppvpn/ppvpn_<deb version>_amd64.deb       shared by the channels
#   linux/apt/dists/<channel>/{InRelease,Release,Release.gpg}
#   linux/apt/dists/<channel>/main/binary-amd64/Packages{,.gz,.xz}  + by-hash/{SHA256,SHA512}/*
#   linux/rpm/<channel>/x86_64/repodata/*                           repomd.xml(.asc)
#   linux/setup/<setup package>{,.asc}
#   linux/ppvpn-archive-keyring.deb, linux/ppvpn-release.rpm (+ .asc)   the newest setup packages
#   linux/ppvpn.asc                                                 public key
#   linux/<channel>/latest.json                                     the app's update check
#
# The site is rebuilt as a whole on every deployment, so there is no earlier index to add to:
# the apt index lists the newest --keep releases given (their debs are copied into the pool),
# and the dnf repository indexes the newest rpm only. That rpm is not copied: it was signed
# before it became a release asset (sign-linux-rpms.sh), and its <location> in the repository
# metadata carries xml:base="<download-base>/<tag>/", so dnf downloads the release asset.
#
# Usage: scripts/ci/build-linux-repo.sh --channel dev|stable --releases <dir>
#          --download-base <https URL> --site-base <https URL> --out <site dir> [--keep 5]
#
#   --releases       one subdirectory per release, named by its tag (desktop-vX.Y.Z, or
#                    desktop-vX.Y.Z-dev.N on dev), each holding release-meta-linux-x64-deb.json,
#                    the deb, release-meta-linux-x64-rpm.json, the signed rpm, and possibly the
#                    setup packages (ppvpn-archive-keyring_*_all.deb, ppvpn-release-*.noarch.rpm,
#                    the latter signed). Nothing else may be in <dir>.
#   --download-base  where release assets are downloaded: <download-base>/<tag>/<file>
#   --site-base      where --out is served: the deb's address in latest.json is under it
#   --out            the site's directory; it may hold other files. This script owns
#                    <out>/linux/apt, <out>/linux/rpm/<channel>, <out>/linux/<channel>, the
#                    setup packages and the key, and is run once per channel into the same
#                    <out>: the pool is shared and the other channel's files are left alone.
#   --keep           releases in the apt index (default 5)
#
# Releases are ordered by the metadata's build, "<version>.<n>": the version numerically,
# then n. Every release must be of the channel, carry the tag its metadata implies, and every
# package must have its metadata's length and sha256; anything else is refused, and <out> is
# only touched once everything was built, signed and verified. The setup packages come from
# the newest release that has them; when both channels are built, the later run's replace the
# earlier's.
#
# Environment:
#   PKG_GPG_PRIVATE_KEY   ASCII-armored secret signing key (imported into a throwaway
#                         GNUPGHOME removed on exit; never printed)
#   PKG_GPG_PASSPHRASE    its passphrase, if any. A key that has one cannot be used without
#                         it: refused before anything is built, never asked for
#   PKG_GPG_FINGERPRINT   expected signing key (default: the production key)
#   PKG_ALLOW_HTTP=1      accept http:// bases. For the CI self-test only, which serves the
#                         site from the runner itself; a published site is https.
#
# Tools: gpg, gpgv, dpkg, dpkg-deb, apt-ftparchive, createrepo_c, rpm, jq, python3, xz, gzip.
set -euo pipefail

die() { echo "build-linux-repo: $*" >&2; exit 1; }
usage() {
  die "usage: build-linux-repo.sh --channel dev|stable --releases <dir> --download-base <https URL> --site-base <https URL> --out <site dir> [--keep 5]"
}

CHANNEL="" RELEASES="" DOWNLOAD_BASE="" SITE_BASE="" OUT="" KEEP=5
while [[ $# -gt 0 ]]; do
  [[ $# -ge 2 ]] || usage
  case "$1" in
    --channel) CHANNEL="$2" ;;
    --releases) RELEASES="$2" ;;
    --download-base) DOWNLOAD_BASE="$2" ;;
    --site-base) SITE_BASE="$2" ;;
    --out) OUT="$2" ;;
    --keep) KEEP="$2" ;;
    *) usage ;;
  esac
  shift 2
done
[[ -n "$CHANNEL" && -n "$RELEASES" && -n "$DOWNLOAD_BASE" && -n "$SITE_BASE" && -n "$OUT" ]] || usage
case "$CHANNEL" in dev|stable) ;; *) die "channel must be dev or stable, got '$CHANNEL'" ;; esac
[[ "$KEEP" =~ ^[1-9][0-9]*$ ]] || die "--keep must be a positive integer, got '$KEEP'"
[[ -d "$RELEASES" ]] || die "$RELEASES is not a directory"
[[ ! -e "$OUT" || -d "$OUT" ]] || die "$OUT is not a directory"
DOWNLOAD_BASE="${DOWNLOAD_BASE%/}"
SITE_BASE="${SITE_BASE%/}"
for base in "$DOWNLOAD_BASE" "$SITE_BASE"; do
  case "$base" in
    https://?*) ;;
    http://?*) [[ "${PKG_ALLOW_HTTP:-}" == 1 ]] || die "$base is not https (PKG_ALLOW_HTTP=1 is for the self-test)" ;;
    *) die "$base is not an https URL" ;;
  esac
  [[ "$base" =~ ^[A-Za-z0-9:/._~%-]+$ ]] || die "$base has characters a base URL does not need"
done
: "${PKG_GPG_PRIVATE_KEY:?PKG_GPG_PRIVATE_KEY is required}"
FINGERPRINT="$(printf '%s' "${PKG_GPG_FINGERPRINT:-6924CE595D51ED53251B8776C48D5132620C3C86}" | tr -d ' ' | tr 'a-f' 'A-F')"
[[ "$FINGERPRINT" =~ ^[0-9A-F]{40}$ ]] || die "PKG_GPG_FINGERPRINT is not a 40-digit fingerprint"
for tool in gpg gpgv gpgconf dpkg dpkg-deb apt-ftparchive createrepo_c rpm jq python3 xz gzip sha256sum sha512sum; do
  command -v "$tool" >/dev/null 2>&1 || die "$tool is required"
done

WORK="$(mktemp -d)"
export GNUPGHOME="$WORK/gnupg"
cleanup() {
  gpgconf --kill all >/dev/null 2>&1 || true
  rm -rf "$WORK"
}
trap cleanup EXIT
mkdir -m 700 "$GNUPGHOME"

sha256() { sha256sum "$1" | cut -d' ' -f1; }
size() { wc -c <"$1" | tr -d ' '; }

# --- 1. the releases: checked, newest first -------------------------------------------------
# One line per release: tag, version, build, deb file, rpm file, published_at (tab-separated).
PLAN="$WORK/plan.tsv"
python3 - "$RELEASES" "$CHANNEL" >"$PLAN" <<'PY'
import hashlib, json, os, re, sys

releases, channel = sys.argv[1:3]
TAG = re.compile(r"^desktop-v(\d+\.\d+\.\d+)(?:-dev\.(\d+))?$")
FILE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._+-]*$")

def refuse(message):
    sys.exit(f"build-linux-repo: {message}")

def sha256_file(path):
    digest = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()

def load(directory, tag, fmt):
    """The release metadata of one package, checked against the package next to it."""
    name = f"release-meta-linux-x64-{fmt}.json"
    path = os.path.join(directory, name)
    if not os.path.isfile(path):
        refuse(f"{tag}: {name} is missing")
    try:
        with open(path, encoding="utf-8") as f:
            meta = json.load(f)
    except ValueError as error:
        refuse(f"{tag}: {name} is not JSON ({error})")
    if not isinstance(meta, dict) or meta.get("schema") != 1:
        refuse(f"{tag}: {name}: unsupported release-meta schema")
    if meta.get("platform") != f"linux-x64-{fmt}":
        refuse(f"{tag}: {name} is for {meta.get('platform')!r}")
    if meta.get("channel") != channel:
        refuse(f"{tag}: {name} was built for channel {meta.get('channel')!r}, not {channel}")
    for key in ("version", "build", "file", "sha256", "published_at"):
        if not isinstance(meta.get(key), str) or not meta[key]:
            refuse(f"{tag}: {name}: {key} is missing")
    if not isinstance(meta.get("length"), int) or isinstance(meta.get("length"), bool):
        refuse(f"{tag}: {name}: length is not an integer")
    if not FILE.match(meta["file"]) or not meta["file"].endswith("." + fmt):
        refuse(f"{tag}: {name}: file is not a bare .{fmt} file name")
    if not re.match(r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ$", meta["published_at"]):
        refuse(f"{tag}: {name}: published_at is not a UTC timestamp")
    package = os.path.join(directory, meta["file"])
    if not os.path.isfile(package):
        refuse(f"{tag}: {meta['file']} is missing")
    if os.path.getsize(package) != meta["length"]:
        refuse(f"{tag}: {meta['file']} is not {meta['length']} bytes")
    if sha256_file(package) != meta["sha256"]:
        refuse(f"{tag}: {meta['file']} does not have the metadata's SHA-256")
    return meta

found = []
for tag in sorted(os.listdir(releases)):
    directory = os.path.join(releases, tag)
    match = TAG.match(tag)
    if not match or not os.path.isdir(directory):
        refuse(f"{directory} is not a release directory named by its tag")
    deb, rpm = load(directory, tag, "deb"), load(directory, tag, "rpm")
    for key in ("version", "build"):
        if deb[key] != rpm[key]:
            refuse(f"{tag}: the deb and the rpm disagree on {key}")
    version, build = deb["version"], deb["build"]
    if not re.match(r"^\d+\.\d+\.\d+$", version):
        refuse(f"{tag}: version {version!r} is not x.y.z")
    number = build[len(version) + 1:]
    if not build.startswith(version + ".") or not number.isdigit():
        refuse(f"{tag}: build {build!r} is not {version}.<number>")
    expected = f"desktop-v{version}" if channel == "stable" else f"desktop-v{version}-dev.{int(number)}"
    if tag != expected:
        refuse(f"{tag}: the metadata ({channel}, build {build}) belongs to tag {expected}")
    order = tuple(int(part) for part in version.split(".")) + (int(number),)
    found.append((order, tag, version, build, deb["file"], rpm["file"], deb["published_at"]))
if not found:
    refuse(f"{releases} has no release")
found.sort(reverse=True)
for earlier, later in zip(found, found[1:]):
    if earlier[0] == later[0]:
        refuse(f"{earlier[1]} and {later[1]} are the same build")
for entry in found:
    print("\t".join(entry[1:]))
PY

TAGS=() VERSIONS=() BUILDS=() DEBS=() RPMS=() PUBLISHED=()
while IFS=$'\t' read -r tag version build deb rpm published; do
  [[ -n "$tag" && -n "$published" ]] || die "could not read the release plan"
  TAGS+=("$tag"); VERSIONS+=("$version"); BUILDS+=("$build"); DEBS+=("$deb"); RPMS+=("$rpm"); PUBLISHED+=("$published")
done <"$PLAN"
[[ ${#TAGS[@]} -gt 0 ]] || die "$RELEASES has no release"

# --- 2. the signing key ---------------------------------------------------------------------
# Every gpg that may use the secret key reads its passphrase from this file (empty when there
# is none) through a loopback pinentry, and so never falls back to an interactive one.
PASSFILE="$GNUPGHOME/passphrase"
(umask 077 && printf '%s' "${PKG_GPG_PASSPHRASE:-}" >"$PASSFILE")
GPG_SECRET=(--batch --pinentry-mode loopback --passphrase-file "$PASSFILE")
printf '%s\n' "$PKG_GPG_PRIVATE_KEY" | gpg "${GPG_SECRET[@]}" --quiet --import 2>/dev/null \
  || die "could not import PKG_GPG_PRIVATE_KEY"
SECRET_KEYS="$(gpg --batch --list-secret-keys --with-colons | awk -F: '$1=="fpr"{print $10}')"
grep -qx "$FINGERPRINT" <<<"$SECRET_KEYS" || die "the signing key is not $FINGERPRINT"
SIGN=(gpg "${GPG_SECRET[@]}" --yes --local-user "$FINGERPRINT")
# The key must be usable before anything is touched. gpg never gets to ask for a passphrase
# (there is nobody to answer on a runner): it reads the file, or fails.
printf 'ppvpn\n' >"$WORK/probe"
if ! "${SIGN[@]}" --armor --detach-sign -o "$WORK/probe.asc" "$WORK/probe" 2>"$WORK/probe.err"; then
  [[ -n "${PKG_GPG_PASSPHRASE:-}" ]] \
    || die "the signing key cannot be used: PKG_GPG_PASSPHRASE is missing (the key has a passphrase)"
  cat "$WORK/probe.err" >&2
  die "the signing key cannot be used: PKG_GPG_PASSPHRASE is wrong (or the key cannot sign)"
fi
gpg --batch --armor --export "$FINGERPRINT" >"$WORK/ppvpn.asc"
gpg --batch --export "$FINGERPRINT" >"$WORK/keyring.gpg"
[[ -s "$WORK/ppvpn.asc" && -s "$WORK/keyring.gpg" ]] || die "could not export the public key"
RPMDB="$WORK/rpmdb"
mkdir -p "$RPMDB"
rpm --dbpath "$RPMDB" --import "$WORK/ppvpn.asc"

# The other channel, built into the same <out>, must have been signed by this key.
if [[ -e "$OUT/linux/ppvpn.asc" ]]; then
  existing="$(gpg --batch --show-keys --with-colons "$OUT/linux/ppvpn.asc" | awk -F: '$1=="fpr"{print $10; exit}')"
  [[ "$existing" == "$FINGERPRINT" ]] || die "$OUT/linux/ppvpn.asc is another key: one site, one key"
fi

check_rpm() {
  local verdict
  verdict="$(rpm --dbpath "$RPMDB" -K "$1")" || die "$1: signature check failed"
  grep -q 'digests signatures OK' <<<"$verdict" \
    || die "$1 is not signed by $FINGERPRINT (sign-linux-rpms.sh signs a release's rpms)"
}
detach_sign() {
  "${SIGN[@]}" --armor --detach-sign -o "$2" "$1"
  gpgv --keyring "$WORK/keyring.gpg" "$2" "$1" 2>/dev/null || die "$2 does not verify"
}

STAGE="$WORK/linux"
mkdir -p "$STAGE"

# --- 3. apt ---------------------------------------------------------------------------------
POOL="pool/main/p/ppvpn"
APT="$STAGE/apt"
DIST="$APT/dists/$CHANNEL"
BIN="$DIST/main/binary-amd64"
mkdir -p "$BIN" "$APT/$POOL"
KEPT=${#TAGS[@]}
[[ "$KEPT" -le "$KEEP" ]] || KEPT="$KEEP"
DEB_VERSION="" DEB_NAME="" previous=""
: >"$WORK/order"
for ((i = 0; i < KEPT; i++)); do
  deb="$RELEASES/${TAGS[i]}/${DEBS[i]}"
  [[ "$(dpkg-deb -f "$deb" Package)" == ppvpn && "$(dpkg-deb -f "$deb" Architecture)" == amd64 ]] \
    || die "$deb is not ppvpn amd64"
  version="$(dpkg-deb -f "$deb" Version)"
  [[ "$version" =~ ^[0-9][A-Za-z0-9.+~-]*$ ]] || die "$deb: version '$version' cannot name a pool file"
  # apt installs the highest version: it must be the release this script calls the newest.
  if [[ -n "$previous" ]]; then
    dpkg --compare-versions "$version" lt "$previous" \
      || die "${TAGS[i]}: deb version $version is not older than $previous, of the release after it"
  fi
  previous="$version"
  name="ppvpn_${version}_amd64.deb"
  # The pool is shared by the channels and its files never change.
  if [[ -e "$OUT/linux/apt/$POOL/$name" ]]; then
    [[ "$(sha256 "$OUT/linux/apt/$POOL/$name")" == "$(sha256 "$deb")" ]] \
      || die "$OUT/linux/apt/$POOL/$name exists with other content than ${TAGS[i]}'s"
  fi
  cp "$deb" "$APT/$POOL/$name"
  printf '%s\n' "$POOL/$name" >>"$WORK/order"
  if [[ "$i" -eq 0 ]]; then
    DEB_VERSION="$version"
    DEB_NAME="$name"
  fi
done

# Filename is relative to linux/apt; stanzas newest first.
(cd "$APT" && apt-ftparchive packages pool) >"$WORK/Packages.unsorted"
python3 - "$WORK/Packages.unsorted" "$WORK/order" >"$BIN/Packages" <<'PY'
import sys

def field(stanza, name):
    for line in stanza.splitlines():
        if line.startswith(name + ": "):
            return line[len(name) + 2:].strip()
    return ""

text = open(sys.argv[1], encoding="utf-8").read().strip()
stanzas = [s for s in text.split("\n\n") if s.strip()] if text else []
order = [line.strip() for line in open(sys.argv[2], encoding="utf-8") if line.strip()]
by_file = {field(s, "Filename"): s for s in stanzas}
if len(by_file) != len(stanzas) or sorted(by_file) != sorted(order):
    sys.exit(f"build-linux-repo: apt-ftparchive indexed {sorted(by_file)}, expected {sorted(order)}")
for name in order:
    if field(by_file[name], "Package") != "ppvpn" or not field(by_file[name], "SHA256"):
        sys.exit(f"build-linux-repo: the stanza of {name} is not a ppvpn package with a SHA256")
sys.stdout.write("\n\n".join(by_file[name] for name in order) + "\n\n")
PY
gzip -9nk "$BIN/Packages"
xz -9k "$BIN/Packages"
(cd "$DIST" && apt-ftparchive \
  -o APT::FTPArchive::Release::Origin=PPVPN \
  -o APT::FTPArchive::Release::Label=PPVPN \
  -o APT::FTPArchive::Release::Suite="$CHANNEL" \
  -o APT::FTPArchive::Release::Codename="$CHANNEL" \
  -o APT::FTPArchive::Release::Architectures=amd64 \
  -o APT::FTPArchive::Release::Components=main \
  -o APT::FTPArchive::Release::Description="PPVPN desktop client ($CHANNEL)" \
  -o APT::FTPArchive::Release::Acquire-By-Hash=yes \
  release . >"$WORK/Release")
mv "$WORK/Release" "$DIST/Release"
grep -q 'main/binary-amd64/Packages.xz$' "$DIST/Release" || die "Release does not list the package index"
"${SIGN[@]}" --clearsign -o "$DIST/InRelease" "$DIST/Release"
"${SIGN[@]}" --armor --detach-sign -o "$DIST/Release.gpg" "$DIST/Release"
gpgv --keyring "$WORK/keyring.gpg" "$DIST/InRelease" 2>/dev/null || die "InRelease does not verify"
gpgv --keyring "$WORK/keyring.gpg" "$DIST/Release.gpg" "$DIST/Release" 2>/dev/null || die "Release.gpg does not verify"
# by-hash copies: clients fetch indexes by their hash (the strongest one Release lists, so
# SHA512 for apt), so a Release and its indexes always match.
for index in Packages Packages.gz Packages.xz; do
  mkdir -p "$BIN/by-hash/SHA256" "$BIN/by-hash/SHA512"
  cp "$BIN/$index" "$BIN/by-hash/SHA256/$(sha256sum "$BIN/$index" | cut -d' ' -f1)"
  cp "$BIN/$index" "$BIN/by-hash/SHA512/$(sha512sum "$BIN/$index" | cut -d' ' -f1)"
done

# --- 4. dnf ---------------------------------------------------------------------------------
# The newest release's rpm, indexed under its release asset's name and left where it is: the
# repository metadata gives its address.
RPM_FILE="${RPMS[0]}"
RPM="$RELEASES/${TAGS[0]}/$RPM_FILE"
RPM_BASE="$DOWNLOAD_BASE/${TAGS[0]}/"
[[ "$(rpm --dbpath "$RPMDB" -qp --qf '%{NAME} %{ARCH}' "$RPM")" == "ppvpn x86_64" ]] \
  || die "$RPM is not ppvpn x86_64"
check_rpm "$RPM"
RPMREPO="$WORK/rpmrepo"
mkdir -p "$RPMREPO"
cp "$RPM" "$RPMREPO/$RPM_FILE"
createrepo_c --quiet --no-database --general-compress-type=gz --baseurl "$RPM_BASE" "$RPMREPO" >/dev/null
python3 - "$RPMREPO" "$RPM_FILE" "$RPM_BASE" <<'PY'
import gzip, os, sys
import xml.etree.ElementTree as ET

repo, rpm, base = sys.argv[1:4]
REPO = "{http://linux.duke.edu/metadata/repo}"
COMMON = "{http://linux.duke.edu/metadata/common}"

def refuse(message):
    sys.exit(f"build-linux-repo: dnf metadata: {message}")

primary = [
    data.find(REPO + "location").get("href")
    for data in ET.parse(os.path.join(repo, "repodata", "repomd.xml")).getroot().findall(REPO + "data")
    if data.get("type") == "primary"
]
if len(primary) != 1 or not primary[0].endswith(".xml.gz"):
    refuse(f"repomd.xml lists {primary} as primary")
with gzip.open(os.path.join(repo, primary[0])) as f:
    packages = ET.parse(f).getroot().findall(COMMON + "package")
if len(packages) != 1 or packages[0].findtext(COMMON + "name") != "ppvpn":
    refuse("primary.xml does not list exactly the ppvpn package")
location = packages[0].find(COMMON + "location")
got = (location.get("{http://www.w3.org/XML/1998/namespace}base"), location.get("href"))
if got != (base, rpm):
    refuse(f"the package's location is {got}, expected {(base, rpm)}")
PY
mkdir -p "$STAGE/rpm/$CHANNEL/x86_64"
mv "$RPMREPO/repodata" "$STAGE/rpm/$CHANNEL/x86_64/repodata"
detach_sign "$STAGE/rpm/$CHANNEL/x86_64/repodata/repomd.xml" "$STAGE/rpm/$CHANNEL/x86_64/repodata/repomd.xml.asc"

# --- 5. setup packages (optional in a release) ----------------------------------------------
# Each from the newest release that has one; published under its name and under the stable
# alias the install instructions use, both with a detached signature.
mkdir -p "$STAGE/setup"
newest_with() {   # file name pattern
  local tag file
  for tag in "${TAGS[@]}"; do
    file="$(find "$RELEASES/$tag" -maxdepth 1 -type f -name "$1" | sort -V | tail -1)"
    if [[ -n "$file" ]]; then
      echo "$file"
      return 0
    fi
  done
}
setup_package() {   # file, alias
  local name
  name="$(basename "$1")"
  [[ "$name" =~ ^[A-Za-z0-9][A-Za-z0-9._+~-]*$ ]] || die "$1: unexpected file name"
  cp "$1" "$STAGE/setup/$name"
  detach_sign "$STAGE/setup/$name" "$STAGE/setup/$name.asc"
  cp "$STAGE/setup/$name" "$STAGE/$2"
  cp "$STAGE/setup/$name.asc" "$STAGE/$2.asc"
}
keyring_deb="$(newest_with 'ppvpn-archive-keyring_*_all.deb')"
release_rpm="$(newest_with 'ppvpn-release-*.noarch.rpm')"
if [[ -n "$keyring_deb" ]]; then
  [[ "$(dpkg-deb -f "$keyring_deb" Package)" == ppvpn-archive-keyring ]] || die "$keyring_deb is not ppvpn-archive-keyring"
  setup_package "$keyring_deb" ppvpn-archive-keyring.deb
fi
if [[ -n "$release_rpm" ]]; then
  [[ "$(rpm --dbpath "$RPMDB" -qp --qf '%{NAME} %{ARCH}' "$release_rpm")" == "ppvpn-release noarch" ]] \
    || die "$release_rpm is not ppvpn-release noarch"
  check_rpm "$release_rpm"
  setup_package "$release_rpm" ppvpn-release.rpm
fi
cp "$WORK/ppvpn.asc" "$STAGE/ppvpn.asc"

# --- 6. latest.json -------------------------------------------------------------------------
# published_at is the newest release's own, so rebuilding the site does not change the file.
NEWEST_DEB="$APT/$POOL/$DEB_NAME"
mkdir -p "$STAGE/$CHANNEL"
jq -n --arg version "${VERSIONS[0]}" --arg build "${BUILDS[0]}" --arg channel "$CHANNEL" \
  --arg deb_url "$SITE_BASE/linux/apt/$POOL/$DEB_NAME" --arg deb_sha "$(sha256 "$NEWEST_DEB")" \
  --argjson deb_size "$(size "$NEWEST_DEB")" --arg deb_version "$DEB_VERSION" \
  --arg rpm_url "$RPM_BASE$RPM_FILE" --arg rpm_sha "$(sha256 "$RPM")" \
  --argjson rpm_size "$(size "$RPM")" --arg rpm_name "$RPM_FILE" \
  --arg published "${PUBLISHED[0]}" \
  '{schema: 1, channel: $channel, version: $version, build: $build,
    deb: {version: $deb_version, url: $deb_url, sha256: $deb_sha, size: $deb_size},
    rpm: {file: $rpm_name, url: $rpm_url, sha256: $rpm_sha, size: $rpm_size},
    published_at: $published}' >"$STAGE/$CHANNEL/latest.json"

# --- 7. into the site: packages, then the indexes, latest.json last -------------------------
LINUX="$OUT/linux"
mkdir -p "$LINUX/apt/$POOL" "$LINUX/apt/dists" "$LINUX/rpm" "$LINUX/setup"
for deb in "$APT/$POOL"/*.deb; do
  name="$(basename "$deb")"
  if [[ -e "$LINUX/apt/$POOL/$name" ]]; then
    echo "reuse linux/apt/$POOL/$name"
  else
    cp "$deb" "$LINUX/apt/$POOL/$name.part"
    mv "$LINUX/apt/$POOL/$name.part" "$LINUX/apt/$POOL/$name"
  fi
done
for file in "$STAGE/setup"/*; do
  [[ -f "$file" ]] || continue   # no setup package in these releases
  cp "$file" "$LINUX/setup/"
done
for alias in ppvpn-archive-keyring.deb ppvpn-release.rpm; do
  if [[ -f "$STAGE/$alias" ]]; then
    cp "$STAGE/$alias" "$STAGE/$alias.asc" "$LINUX/"
  fi
done
cp "$STAGE/ppvpn.asc" "$LINUX/ppvpn.asc"
rm -rf "$LINUX/apt/dists/$CHANNEL" "$LINUX/rpm/$CHANNEL" "${LINUX:?}/$CHANNEL"
mv "$DIST" "$LINUX/apt/dists/$CHANNEL"
mv "$STAGE/rpm/$CHANNEL" "$LINUX/rpm/$CHANNEL"
mv "$STAGE/$CHANNEL" "$LINUX/$CHANNEL"

echo "built the $CHANNEL apt and dnf repositories in $LINUX: ppvpn ${BUILDS[0]} (${TAGS[0]}), $KEPT in the apt index"
