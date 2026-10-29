"""Installs one system image the way sdkmanager would, without its
downloader (.github/actions/android-sdk): sdkmanager reported two truncated
downloads of the 1.5 GB image as warnings and exited 0 ("Premature EOF",
"Error reading Zip content"). Here curl fetches the archive the SDK
repository lists, resuming a broken transfer where it stopped, and the
archive is unpacked only when its size and SHA-1 match the repository's.
The package.xml sdkmanager would write comes from the same listing, so
sdkmanager (and the emulator runner after it) sees the package installed.

    python3 system-image.py <sdk> <api-level> <target> <arch>
"""

import hashlib
import os
import re
import shutil
import subprocess
import sys
import tempfile
import zipfile

REPOSITORY = "https://dl.google.com/android/repository/sys-img/{target}/"
LISTING = "sys-img2-4.xml"
NAMESPACE = "http://schemas.android.com/sdk/android/repo/sys-img2/04"


def fetch(url, path):
    # --retry with -C - continues the same transfer from the bytes on disk.
    subprocess.run(
        ["curl", "-fsSL", "--retry", "5", "--retry-all-errors", "--retry-delay", "5",
         "-C", "-", "-o", path, url],
        check=True,
    )


def sha1(path):
    digest = hashlib.sha1()
    with open(path, "rb") as file:
        for chunk in iter(lambda: file.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def main():
    sdk, api, target, arch = sys.argv[1:]
    package = f"system-images;android-{api};{target};{arch}"
    base = REPOSITORY.format(target=target)
    work = tempfile.mkdtemp(dir=os.environ.get("RUNNER_TEMP"))
    listing = os.path.join(work, LISTING)
    fetch(base + LISTING, listing)
    text = open(listing, encoding="utf-8").read()
    match = re.search(rf'<remotePackage path="{re.escape(package)}">(.*?)</remotePackage>', text, re.S)
    if not match:
        sys.exit(f"{base}{LISTING}: no {package}")
    body = match.group(1)
    complete = re.search(
        r"<complete>\s*<size>(\d+)</size>\s*<checksum type=\"sha1\">([0-9a-f]{40})</checksum>\s*<url>([^<]+)</url>",
        body)
    if not complete:
        sys.exit(f"{package}: no complete archive in the listing")
    size, checksum, name = int(complete.group(1)), complete.group(2), complete.group(3)

    archive = os.path.join(work, name)
    print(f"{package}: {base}{name}, {size} bytes")
    fetch(base + name, archive)
    if os.path.getsize(archive) != size:
        sys.exit(f"{name}: {os.path.getsize(archive)} bytes, the listing says {size}")
    if sha1(archive) != checksum:
        sys.exit(f"{name}: SHA-1 {sha1(archive)}, the listing says {checksum}")

    # The archive holds one directory, the ABI's; its contents are the package.
    dest = os.path.join(sdk, *package.split(";"))
    unpacked = os.path.join(work, "unpacked")
    with zipfile.ZipFile(archive) as zip:
        zip.extractall(unpacked)
    os.remove(archive)
    tops = os.listdir(unpacked)
    if len(tops) != 1:
        sys.exit(f"{name}: {len(tops)} top-level entries, expected one directory")
    shutil.rmtree(dest, ignore_errors=True)
    os.makedirs(os.path.dirname(dest), exist_ok=True)
    shutil.move(os.path.join(unpacked, tops[0]), dest)
    # zipfile drops the permission bits; the emulator's tools need none,
    # but keep the files readable as sdkmanager leaves them.
    subprocess.run(["chmod", "-R", "u+rwX,go+rX", dest], check=True)

    # The listed package minus what only a remote one has (its archives and
    # channel), as a local one with the licence it refers to, last: a
    # package.xml marks the package complete.
    local = re.sub(r"\s*<archives>.*?</archives>|\s*<channelRef[^>]*/>", "", body, flags=re.S)
    licences = "".join(
        licence.group(0)
        for ref in set(re.findall(r'<uses-license ref="([^"]+)"', local))
        for licence in [re.search(rf'<license id="{re.escape(ref)}"[^>]*>.*?</license>', text, re.S)]
        if licence)
    xml = (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
        '<ns2:repository xmlns:ns2="http://schemas.android.com/repository/android/common/02"'
        f' xmlns:sys-img="{NAMESPACE}" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">'
        f'{licences}<localPackage path="{package}" obsolete="false">{local}</localPackage>'
        "</ns2:repository>\n")
    with open(os.path.join(dest, "package.xml"), "w", encoding="utf-8") as file:
        file.write(xml)
    shutil.rmtree(work)
    print(f"{package}: installed in {dest}")


main()
