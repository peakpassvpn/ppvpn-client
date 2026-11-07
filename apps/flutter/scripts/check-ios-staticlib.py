#!/usr/bin/env python3
"""Check the exact bridge archive before Xcode links it into any configuration."""
import argparse
from pathlib import Path
import re
import subprocess

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('library', type=Path)
parser.add_argument('exports', type=Path)
args = parser.parse_args()
architectures = subprocess.check_output(['xcrun', 'lipo', '-archs', str(args.library)], text=True).split()
if architectures != ['arm64']:
    raise SystemExit('bridge static archive must contain exactly arm64')
result = subprocess.run(['nm', '-gU', str(args.library)], text=True, capture_output=True)
# Apple nm can diagnose newer LLVM bitcode in unrelated Rust archive members.
# Require actual defined symbol rows, not an empty/successful nm invocation.
actual = {match.group(1) for line in result.stdout.splitlines()
          if (match := re.match(r'^\s*[0-9a-fA-F]+\s+[A-Za-z]\s+(_\w+)\s*$', line))}
required = {line.strip() for line in args.exports.read_text().splitlines()
            if line.strip() and not line.lstrip().startswith('#')}
if result.returncode not in (0, 1) or not required or required - actual:
    raise SystemExit('bridge static archive lacks required exports: ' + ', '.join(sorted(required - actual)))
print(f'arm64 bridge static archive: {len(required)} required symbols verified')
