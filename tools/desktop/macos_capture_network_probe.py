#!/usr/bin/env python3
"""Read-only, fixed-endpoint network diagnostics for isolated native QA."""
import json
import pathlib
import re
import socket
import ssl
import struct
import sys

QUERY = b'\x71\x21\x01\x00\x00\x01\x00\x00\x00\x00\x00\x00\x07example\x03com\x00\x00\x01\x00\x01'


def read_exact(stream, count):
    data = b''
    while len(data) < count:
        chunk = stream.recv(count - len(data))
        if not chunk:
            raise EOFError()
        data += chunk
    return data


def dns_reply(stream):
    stream.sendall(struct.pack('!H', len(QUERY)) + QUERY)
    answer = read_exact(stream, struct.unpack('!H', read_exact(stream, 2))[0])
    if len(answer) < 12:
        raise ValueError('short DNS reply')
    return dict(matching_id=answer[:2] == QUERY[:2], rcode=answer[3] & 15,
                answers=struct.unpack('!H', answer[6:8])[0])


def measure(out, stage):
    rows = []
    def attempt(kind, endpoint, operation):
        row = dict(kind=kind, endpoint=endpoint)
        try:
            row.update(operation())
        except Exception as exc:
            row.update(error=type(exc).__name__, errno=getattr(exc, 'errno', None))
        rows.append(row)

    def http():
        with socket.create_connection(('1.1.1.1', 80), timeout=2) as stream:
            stream.sendall(b'GET / HTTP/1.0\r\nHost: one.one.one.one\r\n\r\n')
            return dict(http_header=stream.recv(16).startswith(b'HTTP/1.'))
    attempt('http', '1.1.1.1:80', http)
    # These are the production proxy-mode DoT endpoints. Direct-mode local DNS
    # is measured separately; no endpoint here replaces its system resolver.
    for address, hostname in [('1.1.1.1', 'cloudflare-dns.com'),
                              ('8.8.8.8', 'dns.google'),
                              ('9.9.9.9', 'dns.quad9.net')]:
        def dot(address=address, hostname=hostname):
            context = ssl.create_default_context()
            with socket.create_connection((address, 853), timeout=2) as stream:
                with context.wrap_socket(stream, server_hostname=hostname) as tls:
                    return dict(tls_verified=True, **dns_reply(tls))
        attempt('dot', address + ':853', dot)
    physical = (out / 'dns-before.txt').read_text()
    servers = dict.fromkeys(re.findall(r'nameserver\[\d+\]\s*:\s*(\S+)', physical))
    for server in servers:
        for transport in ['udp', 'tcp']:
            def local(server=server, transport=transport):
                family = socket.AF_INET6 if ':' in server else socket.AF_INET
                with socket.socket(family, socket.SOCK_DGRAM if transport == 'udp' else socket.SOCK_STREAM) as stream:
                    stream.settimeout(2)
                    stream.connect((server, 53))
                    if transport == 'tcp':
                        return dns_reply(stream)
                    stream.send(QUERY)
                    answer = stream.recv(65535)
                    if len(answer) < 12:
                        raise ValueError('short DNS reply')
                    return dict(matching_id=answer[:2] == QUERY[:2], rcode=answer[3] & 15,
                                answers=struct.unpack('!H', answer[6:8])[0])
            attempt('physical-dns-' + transport, server + ':53', local)
    (out / ('network-' + stage + '.json')).write_text(json.dumps(rows, indent=2) + '\n')


if __name__ == '__main__':
    stage = sys.argv[2]
    if stage not in ['before', 'attached']:
        raise ValueError('invalid stage')
    measure(pathlib.Path(sys.argv[1]), stage)
