"""Controlled loopback feed for the actual updater UI test; no install occurs."""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import time
import sys
class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        time.sleep(3)  # exercise check-to-display foreground changes
        body = b'''<?xml version="1.0"?><rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle"><channel><title>QA</title><item><title>PPVPN QA</title><enclosure url="https://example.com/not-installed.exe" sparkle:version="0.7.5.2" sparkle:shortVersionString="0.7.5" length="1" type="application/octet-stream" /></item></channel></rss>'''
        if '--macos' in sys.argv:
            body = body.replace(b'not-installed.exe', b'not-installed.zip')
        self.send_response(200)
        self.send_header('Content-Type','application/xml')
        self.send_header('Content-Length',str(len(body)))
        self.end_headers()
        self.wfile.write(body)
if __name__ == '__main__':
    ThreadingHTTPServer(('127.0.0.1',18763),Handler).serve_forever()
