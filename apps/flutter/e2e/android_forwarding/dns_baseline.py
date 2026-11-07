#!/usr/bin/env python3
"""Diagnostic production DoT baselines; never an acceptance substitute.

No CA overrides, insecure TLS, alternate resolver or transport. Bound each
member by the same 3s attempt budget used by Core's dns-remote configuration.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
import json
from pathlib import Path
import socket
import ssl
import struct
import time

RESOLVERS = ('1.1.1.1', '8.8.8.8', '9.9.9.9')
QUERY = bytes.fromhex('5d1a01000001000000000000') + b'\x07example\x03com\0\0\x01\0\x01'


def probe(server, timeout=3):
    started = time.monotonic()
    deadline = started + timeout
    result = {'server': server, 'port': 853, 'query': 'example.com A',
              'tls_peer_identity': server, 'tcp_853': False, 'ok': False,
              'stage': 'tcp_connect', 'trust': 'Python/system CA verification'}

    def remaining():
        left = deadline - time.monotonic()
        if left <= 0:
            raise TimeoutError('DoT member attempt budget exhausted')
        return left

    def read_exact(stream, size):
        data = bytearray()
        while len(data) < size:
            stream.settimeout(remaining())
            part = stream.recv(size - len(data))
            if not part:
                raise EOFError('DoT peer closed before complete DNS frame')
            data.extend(part)
        return bytes(data)

    try:
        with socket.create_connection((server, 853), timeout=remaining()) as raw:
            result.update(tcp_853=True, local_address=raw.getsockname(),
                          peer_address=raw.getpeername(), stage='tls_handshake')
            raw.settimeout(remaining())
            # IP literal identity, as Core's TLS server config supplies. Python
            # verifies its IP SAN and does not invent a DNS hostname/SNI.
            context = ssl.create_default_context()
            with context.wrap_socket(raw, server_hostname=server) as tls:
                result.update(tls_version=tls.version(), cipher=tls.cipher(), stage='dns_exchange')
                tls.settimeout(remaining())
                tls.sendall(struct.pack('!H', len(QUERY)) + QUERY)
                size = struct.unpack('!H', read_exact(tls, 2))[0]
                if size < 12 or size > 65535:
                    raise ValueError('Invalid DNS response frame length')
                reply = read_exact(tls, size)
                ident, flags, _, answers, _, _ = struct.unpack('!6H', reply[:12])
                if ident != 0x5d1a or not flags & 0x8000:
                    raise ValueError('Uncorrelated DNS response')
                result.update(rcode=flags & 15, answers=answers,
                              ok=(flags & 15) == 0 and answers > 0, stage='complete')
    except Exception as error:
        result.update(error_type=type(error).__name__, error=str(error),
                      errno=getattr(error, 'errno', None),
                      verify_code=getattr(error, 'verify_code', None),
                      verify_message=getattr(error, 'verify_message', None))
    result['duration_ms'] = round((time.monotonic() - started) * 1000)
    return result


def baseline():
    with ThreadPoolExecutor(max_workers=3) as pool:
        return {'path': 'Ubuntu host direct egress (also controlled SS node egress)',
                'scope': 'Not emulator/Sail binding or BoringSSL trust-store proof',
                'resolvers': list(pool.map(probe, RESOLVERS))}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--require-tcp', action='store_true')
    args = parser.parse_args()
    result = baseline()
    args.output.write_text(json.dumps(result, indent=2) + '\n')
    if args.require_tcp and not any(row['tcp_853'] for row in result['resolvers']):
        raise SystemExit('All production DoT853 TCP baselines failed; acceptance NOT run/passed')


if __name__ == '__main__':
    main()
