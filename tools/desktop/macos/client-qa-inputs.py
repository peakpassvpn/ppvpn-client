#!/usr/bin/env python3
"""Exact tracked inputs for reusing an unchanged normal client in observer QA."""
import subprocess
import sys

# CI routing and the observer may change without changing the client under test.
# Everything else is compared conservatively, including unrelated tracked files.
excluded = {
    b'tools/desktop/macos/update-prompt-qa.swift',
    b'tools/desktop/macos/run-update-prompt-qa.sh',
    b'.github/workflows/ci.yml',
    b'.github/workflows/update-notification-qa.yml',
}
for entry in subprocess.check_output(['git', 'ls-tree', '-rz', '--full-tree', 'HEAD']).split(b'\0'):
    if not entry:
        continue
    metadata, path = entry.split(b'\t', 1)
    if path in excluded:
        continue
    sys.stdout.buffer.write(metadata + b'\t' + path + b'\n')
