#!/usr/bin/env python3
"""Lab target: answers who connected (the exit), over HTTP :80 and HTTPS :443."""
import http.server, socketserver, ssl, sys, threading, time

class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *args): pass
    def do_HEAD(self): self.do_GET()
    def do_GET(self):
        if self.path.startswith("/generate_204"):
            self.send_response(204); self.send_header("Content-Length", "0"); self.end_headers(); return
        if self.path.startswith("/drip"):
            # /drip?seconds: a chunk of 1 KiB every 100 ms.
            seconds = float(self.path.partition("?")[2] or 3)
            self.send_response(200); self.send_header("Transfer-Encoding", "chunked"); self.end_headers()
            end = time.time() + seconds
            try:
                while time.time() < end:
                    self.wfile.write(b"400\r\n" + b"x" * 1024 + b"\r\n"); self.wfile.flush(); time.sleep(0.1)
                self.wfile.write(b"0\r\n\r\n")
            except OSError: pass
            return
        if self.path.startswith("/bytes"):
            n = int(self.path.partition("?")[2] or 1000000)
            self.send_response(200); self.send_header("Content-Length", str(n)); self.end_headers()
            self.wfile.write(b"x" * n); return
        body = ("exit=%s host=%s\n" % (self.client_address[0], self.headers.get("Host", ""))).encode()
        self.send_response(200); self.send_header("Content-Length", str(len(body))); self.end_headers()
        self.wfile.write(body)

class Server(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True; allow_reuse_address = True

def serve(port, tls):
    server = Server(("0.0.0.0", port), Handler)
    if tls:
        ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER); ctx.load_cert_chain(sys.argv[1], sys.argv[2])
        server.socket = ctx.wrap_socket(server.socket, server_side=True)
    server.serve_forever()

def udp_echo():
    import socket
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM); s.bind(("0.0.0.0", 9999))
    while True:
        data, peer = s.recvfrom(2048); s.sendto(("exit=%s " % peer[0]).encode() + data, peer)

threading.Thread(target=udp_echo, daemon=True).start()
threading.Thread(target=serve, args=(80, False), daemon=True).start()
serve(443, True)
