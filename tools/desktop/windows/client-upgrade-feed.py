"""Loopback TLS appcast for real client A-to-B QA. Serves only the test B package."""
import argparse, hashlib, html, http.server, json, pathlib, ssl, time
p=argparse.ArgumentParser()
p.add_argument('--package-dir',required=True);p.add_argument('--private-dir',required=True);p.add_argument('--log',required=True)
a=p.parse_args();root=pathlib.Path(a.package_dir)
meta=json.loads((root/'release-meta-windows-x64.json').read_text(encoding='utf-8-sig'))
package=root/meta['file']
assert package.stat().st_size==meta['length']
assert hashlib.sha256(package.read_bytes()).hexdigest()==meta['sha256']
assert meta['ed_signature'] and meta['build']=='0.7.5.60001'
class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        received=time.time()
        if self.path.endswith('appcast-windows-x64.xml'):
            time.sleep(3)  # real check-to-display transition, not a fake fullscreen input
            body=('''<?xml version="1.0"?><rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle"><channel><title>PPVPN CI only</title><item><title>PPVPN test B</title><enclosure url="https://localhost:18764/'''+html.escape(meta['file'],quote=True)+'''" sparkle:version="'''+meta['build']+'''" sparkle:shortVersionString="'''+meta['version']+'''" sparkle:edSignature="'''+meta['ed_signature']+'''" sparkle:installerArguments="/S" length="'''+str(meta['length'])+'''" type="application/octet-stream" /></item></channel></rss>''').encode()
            content='application/xml'
        elif self.path=='/'+meta['file']:
            body=package.read_bytes();content='application/octet-stream'
        else:
            self.send_error(404);return
        self.send_response(200);self.send_header('Content-Type',content);self.send_header('Content-Length',str(len(body)));self.end_headers()
        try:self.wfile.write(body);outcome='written'
        except (ConnectionResetError,BrokenPipeError):outcome='client-closed'
        with open(a.log,'a',encoding='utf-8') as f:
            f.write(json.dumps({'received':received,'finished':time.time(),'path':self.path,'bytes':len(body),'outcome':outcome,'target_build':meta['build']})+'\n')
server=http.server.ThreadingHTTPServer(('127.0.0.1',18764),Handler)
context=ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
context.load_cert_chain(str(pathlib.Path(a.private_dir)/'tls.crt'),str(pathlib.Path(a.private_dir)/'tls.key'))
server.socket=context.wrap_socket(server.socket,server_side=True)
server.serve_forever()
