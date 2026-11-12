"""Loopback TLS appcast for real client A-to-B QA. Serves only the test B package."""
import argparse, ctypes, hashlib, html, http.server, json, pathlib, socket, ssl, time
p=argparse.ArgumentParser()
p.add_argument('--package-dir',required=True);p.add_argument('--private-dir',required=True);p.add_argument('--log',required=True)
a=p.parse_args();root=pathlib.Path(a.package_dir)
ctypes.windll.kernel32.GetTickCount64.restype=ctypes.c_ulonglong
def tick():return ctypes.windll.kernel32.GetTickCount64()
meta=json.loads((root/'release-meta-windows-x64.json').read_text(encoding='utf-8-sig'))
package=root/meta['file']
assert package.stat().st_size==meta['length']
assert hashlib.sha256(package.read_bytes()).hexdigest()==meta['sha256']
assert meta['ed_signature'] and meta['build']=='0.7.5.60001'
def peer_pid(port):
    # Associate each real loopback HTTP response with its actual client PID.
    # User-Agent has only the display version in WinSparkle 0.9.2, not build.
    class Row(ctypes.Structure):
        _fields_=[(name,ctypes.c_ulong) for name in ('state','local_addr','local_port','remote_addr','remote_port','pid')]
    size=ctypes.c_ulong(0)
    api=ctypes.windll.iphlpapi.GetExtendedTcpTable
    api(None,ctypes.byref(size),True,2,5,0)
    data=ctypes.create_string_buffer(size.value)
    if api(data,ctypes.byref(size),True,2,5,0):return None
    count=ctypes.c_ulong.from_buffer_copy(data.raw[:4]).value
    for i in range(count):
        row=Row.from_buffer_copy(data.raw[4+i*ctypes.sizeof(Row):4+(i+1)*ctypes.sizeof(Row)])
        if socket.ntohs(row.local_port & 65535)==port and socket.ntohs(row.remote_port & 65535)==18764:return row.pid
    return None

def stage_for(pid):
    path=pathlib.Path(a.log).parent/'client-processes.jsonl'
    if path.exists():
        for line in path.read_text().splitlines():
            try:row=json.loads(line)
            except json.JSONDecodeError:continue
            if row.get('pid')==pid:return row.get('stage')
    return None

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        received=time.time();received_tick=tick();pid=peer_pid(self.client_address[1])
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
        finished_tick=tick();stage=stage_for(pid)
        with open(a.log,'a',encoding='utf-8') as f:
            f.write(json.dumps({'received':received,'finished':time.time(),'received_tick':received_tick,'finished_tick':finished_tick,'path':self.path,'bytes':len(body),'outcome':outcome,'target_build':meta['build'],'client_pid':pid,'client_stage':stage,'user_agent':self.headers.get('User-Agent')})+'\n')
        if stage=='A' and outcome=='written' and self.path.endswith('appcast-windows-x64.xml'):
            marker=pathlib.Path(a.log).parent/'appcast-A-written.txt'
            temporary=marker.with_suffix('.tmp')
            temporary.write_text(f'{pid} {finished_tick}',encoding='ascii')
            temporary.replace(marker)
server=http.server.ThreadingHTTPServer(('127.0.0.1',18764),Handler)
context=ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
context.minimum_version=ssl.TLSVersion.TLSv1_2
context.load_cert_chain(str(pathlib.Path(a.private_dir)/'tls.crt'),str(pathlib.Path(a.private_dir)/'tls.key'))
server.socket=context.wrap_socket(server.socket,server_side=True)
server.serve_forever()
