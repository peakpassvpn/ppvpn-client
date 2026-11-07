import errno
import ssl
import struct
import unittest
from unittest.mock import patch

import dns_baseline as dns


class Stream:
    def __init__(self, reply=b''):
        self.reply = reply
        self.sent = b''
    def __enter__(self): return self
    def __exit__(self, *args): pass
    def settimeout(self, value): pass
    def getsockname(self): return ('192.0.2.2', 12345)
    def getpeername(self): return ('1.1.1.1', 853)
    def version(self): return 'TLSv1.3'
    def cipher(self): return ('test-cipher', 'TLSv1.3', 256)
    def sendall(self, data): self.sent += data
    def recv(self, size):
        # Deliberate short reads to exercise DNS stream framing.
        take = min(size, 3)
        result, self.reply = self.reply[:take], self.reply[take:]
        return result


class BaselineTests(unittest.TestCase):
    def test_tcp_errno_is_preserved_without_claiming_tls(self):
        with patch.object(dns.socket, 'create_connection', side_effect=OSError(errno.ENETUNREACH, 'unreachable')):
            result = dns.probe('1.1.1.1')
        self.assertEqual('tcp_connect', result['stage'])
        self.assertEqual(errno.ENETUNREACH, result['errno'])
        self.assertFalse(result['tcp_853'])
        self.assertFalse(result['ok'])

    def test_certificate_failure_keeps_tcp_success_separate(self):
        with patch.object(dns.socket, 'create_connection', return_value=Stream()), \
             patch.object(dns.ssl, 'create_default_context') as factory:
            factory.return_value.wrap_socket.side_effect = ssl.SSLCertVerificationError('untrusted')
            result = dns.probe('1.1.1.1')
        self.assertTrue(result['tcp_853'])
        self.assertEqual('tls_handshake', result['stage'])
        self.assertEqual('SSLCertVerificationError', result['error_type'])
        self.assertFalse(result['ok'])

    def test_verified_context_and_correlated_framed_response(self):
        reply = struct.pack('!6H', 0x5d1a, 0x8180, 1, 1, 0, 0)
        stream = Stream(struct.pack('!H', len(reply)) + reply)
        with patch.object(dns.socket, 'create_connection', return_value=Stream()), \
             patch.object(dns.ssl, 'create_default_context') as factory:
            factory.return_value.wrap_socket.return_value = stream
            result = dns.probe('1.1.1.1')
            factory.return_value.wrap_socket.assert_called_once()
            self.assertEqual('1.1.1.1', factory.return_value.wrap_socket.call_args.kwargs['server_hostname'])
        self.assertTrue(result['ok'])
        self.assertEqual(struct.pack('!H', len(dns.QUERY)) + dns.QUERY, stream.sent)

    def test_truncated_tls_dns_stream_cannot_look_like_an_answer(self):
        with patch.object(dns.socket, 'create_connection', return_value=Stream()), \
             patch.object(dns.ssl, 'create_default_context') as factory:
            factory.return_value.wrap_socket.return_value = Stream(b'\x00')
            result = dns.probe('1.1.1.1')
        self.assertEqual('dns_exchange', result['stage'])
        self.assertEqual('EOFError', result['error_type'])
        self.assertFalse(result['ok'])


if __name__ == '__main__':
    unittest.main()
