#!/usr/bin/env python3
"""Read-only APK/AAB size inventory. This is not a Play download estimate."""
import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import stat
import struct
import zipfile

ABIS = {'arm64-v8a': (2, 183), 'x86_64': (2, 62), 'armeabi-v7a': (1, 40), 'x86': (1, 3)}
REQUIRED = {'optimization', 'strip', 'signing', 'toolchain', 'build_number'}


def digest(stream):
    value = hashlib.sha256()
    for data in iter(lambda: stream.read(1024 * 1024), b''):
        value.update(data)
    return value.hexdigest()


def inventory(path, kind, variant, abis, commit, baseline, conditions, approved_removals=None):
    if not REQUIRED <= conditions.keys():
        raise ValueError('incomplete build conditions')
    if (kind == 'aab') != (variant == 'bundle'):
        raise ValueError('AAB uses bundle scope; APK uses universal or abi-split scope')
    if variant == 'abi-split' and len(abis) != 1:
        raise ValueError('one ABI is required for an ABI-specific APK')
    files, groups, actual = [], {}, set()
    with zipfile.ZipFile(path) as archive:
        names = archive.namelist()
        if len(set(names)) != len(names):
            raise ValueError('duplicate archive entries')
        for info in archive.infolist():
            name = PurePosixPath(info.filename)
            if name.is_absolute() or '..' in name.parts or stat.S_ISLNK(info.external_attr >> 16):
                raise ValueError('unsafe archive member')
            if info.is_dir():
                continue
            group = 'resources'
            if info.filename.endswith('.dex'):
                group = 'dex'
            if 'lib' in name.parts and info.filename.endswith('.so'):
                position = name.parts.index('lib')
                abi = name.parts[position + 1]
                if abi not in ABIS:
                    raise ValueError('unknown native ABI')
                with archive.open(info) as stream:
                    header = stream.read(20)
                if len(header) < 20 or header[:4] != b'\x7fELF' or header[5] not in (1, 2):
                    raise ValueError('invalid native ELF header')
                machine, = struct.unpack_from(('<' if header[5] == 1 else '>') + 'H', header, 18)
                if (header[4], machine) != ABIS[abi]:
                    raise ValueError('ELF architecture differs from its ABI directory')
                actual.add(abi)
                group = 'native/' + abi
            with archive.open(info) as stream:
                sha = digest(stream)
            files.append({'path': info.filename, 'bytes': info.file_size,
                          'zip_compressed_bytes': info.compress_size, 'sha256': sha, 'component': group})
            groups[group] = groups.get(group, 0) + info.file_size
    if actual != set(abis):
        raise ValueError('actual native ABI set does not match the requested measurement')
    with path.open('rb') as stream:
        sha = digest(stream)
    return {'schema': 1, 'platform': 'android', 'kind': kind, 'variant': variant,
            'source_commit': commit, 'functional_baseline': baseline, 'conditions': conditions,
            'approved_removals': sorted(approved_removals or []), 'actual_abis': sorted(actual),
            'artifact_name': path.name, 'container_bytes': path.stat().st_size, 'sha256': sha,
            'uncompressed_entry_bytes': sum(row['bytes'] for row in files), 'components': groups,
            'files': files, 'play_device_download_bytes': None,
            'note': 'Build/signing provenance supplied by caller. AAB and universal APK bytes are not device download sizes.'}


def compare(a, b):
    def incomplete(value):
        return (value is None or isinstance(value, dict) and any(incomplete(v) for v in value.values())
                or isinstance(value, list) and any(incomplete(v) for v in value))
    if any(incomplete(row['conditions']) for row in (a, b)):
        raise ValueError('unknown build conditions')
    for key in ('schema', 'platform', 'kind', 'variant', 'actual_abis', 'functional_baseline', 'conditions'):
        if a[key] != b[key]:
            raise ValueError('incomparable artifact scope: ' + key)
    delta = a['container_bytes'] - b['container_bytes']
    return {'baseline_commit': a['source_commit'], 'candidate_commit': b['source_commit'],
            'saved_bytes': delta, 'saved_MB': delta / 1_000_000, 'saved_MiB': delta / 1_048_576,
            'component_savings_bytes': {key: a['components'].get(key, 0) - b['components'].get(key, 0)
                for key in sorted(a['components'].keys() | b['components'].keys())},
            'baseline_approved_removals': a.get('approved_removals', []),
            'candidate_approved_removals': b.get('approved_removals', []),
            'note': 'Approved feature removals must be disclosed; savings are not solely runtime deduplication.'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    inv = commands.add_parser('inventory')
    inv.add_argument('--artifact', type=Path, required=True)
    inv.add_argument('--kind', choices=('apk', 'aab'), required=True)
    inv.add_argument('--variant', choices=('universal', 'abi-split', 'bundle'), required=True)
    inv.add_argument('--abis', required=True)
    inv.add_argument('--source-commit', required=True)
    inv.add_argument('--functional-baseline', required=True)
    inv.add_argument('--conditions', type=Path, required=True)
    inv.add_argument('--approved-removal', action='append', default=[], choices=(
        'mobile-local-proxy-server', 'mobile-proxy-settings-ui', 'app-standard-forwarder'))
    cmp = commands.add_parser('compare')
    cmp.add_argument('baseline', type=Path)
    cmp.add_argument('candidate', type=Path)
    for command in (inv, cmp):
        command.add_argument('--output', type=Path)
    args = parser.parse_args()
    try:
        result = (inventory(args.artifact, args.kind, args.variant, args.abis.split(','),
            args.source_commit, args.functional_baseline, json.loads(args.conditions.read_text()),
            args.approved_removal) if args.command == 'inventory' else
            compare(json.loads(args.baseline.read_text()), json.loads(args.candidate.read_text())))
    except (ValueError, OSError, KeyError, IndexError, zipfile.BadZipFile) as error:
        parser.error(str(error))
    data = json.dumps(result, indent=2, sort_keys=True) + '\n'
    if args.output:
        args.output.write_text(data)
    else:
        print(data, end='')


if __name__ == '__main__':
    main()
