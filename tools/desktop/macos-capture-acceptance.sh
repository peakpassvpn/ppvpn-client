#!/usr/bin/env bash
# Destructive only inside a fresh GitHub-hosted macOS runner. Never run on a
# developer workstation: both independent guards are required before sudo.
set -euo pipefail
[[ "${GITHUB_ACTIONS:-}" == true && "${RUNNER_ENVIRONMENT:-}" == github-hosted && "$(uname -s)" == Darwin ]] || {
  echo 'ISOLATED_GITHUB_MACOS_RUNNER_REQUIRED' >&2; exit 2;
}
artifact="${1:?signed artifact directory required}"
out="${2:?evidence directory required}"
mkdir -p "$out"
image=/Library/PrivilegedHelperTools/com.peakpassvpn.ppvpn.service.bundle/Contents/MacOS/ppvpn-service
daemon=com.peakpassvpn.ppvpn.service
agent=com.peakpassvpn.ppvpn.user-runtime
plist=/Library/LaunchDaemons/com.peakpassvpn.ppvpn.service.plist
marker=/private/var/db/ppvpn-capture-acceptance # sensitive-check: allow — canonical macOS isolated acceptance marker
[[ ! -e "$image" && ! -e "$plist" && ! -e "$marker" && ! -e "/Library/LaunchAgents/$agent.plist" ]] || { echo EXISTING_INSTALLATION_REFUSED >&2; exit 2; }
uid="$(id -u)"
[[ "$uid" != 0 ]] || exit 2
python3 - "$artifact" "${GITHUB_SHA:?}" <<'PY'
import hashlib,json,pathlib,sys
p=pathlib.Path(sys.argv[1]); m=json.loads((p/'manifest.json').read_text())
assert m['commit']==sys.argv[2], 'artifact/source commit mismatch'
for name in ['ppvpn-service','ppvpn-service-install','ppvpn-service-uninstall','ppvpn-native-node']:
    assert hashlib.sha256((p/name).read_bytes()).hexdigest()==m['sha256'][name], name
PY
team="$(tr -d '\n' < crates/service/src/macos_team_id)"
verify_signature() {
  local file="$1" identifier="$2" evidence="$3"
  codesign --verify --strict --verbose=2 -R "=anchor apple generic and identifier \"$identifier\" and certificate 1[field.1.2.840.113635.100.6.2.6] and certificate leaf[field.1.2.840.113635.100.6.1.13] and certificate leaf[subject.OU] = \"$team\"" "$file" > "$evidence" 2>&1 || {
    cat "$evidence" >&2
    return 1
  }
}
for name in ppvpn-service ppvpn-service-install ppvpn-service-uninstall; do
  verify_signature "$artifact/$name" "com.peakpassvpn.ppvpn.${name#ppvpn-}" "$out/$name-signature-check.txt"
done
"$artifact/ppvpn-service" --build-info > "$out/build-info.json"
python3 - "$out/build-info.json" <<'PY'
import json,sys
m=json.load(open(sys.argv[1])); assert m['user_runtime'] and m['capture_acceptance']
PY
netstat -rn > "$out/routes-before.txt"
scutil --dns > "$out/dns-before.txt"
route -n get default > "$out/default-before.txt"
ifconfig -l > "$out/interfaces-before.txt"
# Record physical resolver reachability before any capture resource exists.
# This is diagnostic evidence; the native DNS success gate remains unchanged.
python3 - "$out" <<'PY_DNS_BASELINE'
import json,pathlib,re,socket,struct,sys
out=pathlib.Path(sys.argv[1]); text=(out/'dns-before.txt').read_text()
servers=list(dict.fromkeys(re.findall(r'nameserver\[\d+\]\s*:\s*(\S+)',text)))
query=b'\x71\x21\x01\x00\x00\x01\x00\x00\x00\x00\x00\x00\x07example\x03com\x00\x00\x01\x00\x01'
rows=[]
for server in servers:
    for transport in ['udp','tcp']:
        row=dict(server=server,transport=transport)
        try:
            family=socket.AF_INET6 if ':' in server else socket.AF_INET
            with socket.socket(family,socket.SOCK_DGRAM if transport=='udp' else socket.SOCK_STREAM) as sock:
                sock.settimeout(2);sock.connect((server,53))
                if transport=='udp':sock.send(query);answer=sock.recv(65535)
                else:
                    sock.sendall(struct.pack('!H',len(query))+query)
                    def read_exact(n):
                        data=b''
                        while len(data)<n:
                            chunk=sock.recv(n-len(data))
                            if not chunk:raise EOFError()
                            data+=chunk
                        return data
                    answer=read_exact(struct.unpack('!H',read_exact(2))[0])
                row.update(matching_id=answer[:2]==query[:2],rcode=answer[3]&15,answers=struct.unpack('!H',answer[6:8])[0])
        except Exception as exc:row.update(error=type(exc).__name__,errno=getattr(exc,'errno',None))
        rows.append(row)
(out/'physical-dns-baseline.json').write_text(json.dumps(rows,indent=2))
PY_DNS_BASELINE
installed=0
cleanup() {
  set +e
  if (( installed )); then
    sudo /usr/bin/tail -c 65536 /Library/Logs/PPVPN/ppvpn-service.err.log > "$out/root-stderr.txt" 2>&1
    sudo /usr/bin/tail -c 65536 /Library/Logs/PPVPN/ppvpn-service.out.log > "$out/root-stdout.txt" 2>&1
    sudo /bin/launchctl bootout "gui/$uid/$agent" 2>/dev/null
    sudo "$artifact/ppvpn-service-uninstall" > "$out/uninstall.txt" 2>&1
  fi
  sudo rm -f "$marker"
  netstat -rn > "$out/routes-after.txt"
  scutil --dns > "$out/dns-after.txt"
  route -n get default > "$out/default-after.txt"
  ifconfig -l > "$out/interfaces-after.txt"
  [[ ! -e "/Library/Application Support/PPVPN/run/capture/grant.json" ]] || echo BROKER_JOURNAL_LEFTOVER > "$out/cleanup-failed.txt" # sensitive-check: allow — canonical macOS runtime journal
  [[ ! -e "$image" && ! -e "$plist" && ! -e "/Library/LaunchAgents/$agent.plist" ]] || echo INSTALLATION_LEFTOVER >> "$out/cleanup-failed.txt"
}
trap cleanup EXIT
sudo /usr/bin/install -o root -g wheel -m 644 /dev/null "$marker"
installed=1
sudo "$artifact/ppvpn-service-install" > "$out/install.txt" 2>&1
verify_signature "$image" com.peakpassvpn.ppvpn.service "$out/installed-helper-signature-check.txt"
sudo /bin/launchctl print "system/$daemon" > "$out/root-job.txt"
python3 - "$out/directory-metadata.json" <<'PY_METADATA'
import json,os,stat,sys
paths=['/','/Library','/Library/Application Support','/Library/Application Support/PPVPN','/Library/Application Support/PPVPN/run','/Library/Application Support/PPVPN/run/capture','/private','/private/var','/private/var/run'] # sensitive-check: allow — canonical macOS directory permission evidence
rows=[]
for path in paths:
    try:
        m=os.lstat(path)
        rows.append(dict(path=path,uid=m.st_uid,gid=m.st_gid,mode=oct(stat.S_IMODE(m.st_mode)),directory=stat.S_ISDIR(m.st_mode),symlink=stat.S_ISLNK(m.st_mode)))
    except FileNotFoundError: rows.append(dict(path=path,missing=True))
with open(sys.argv[1],'w') as f:json.dump(rows,f,indent=2)
PY_METADATA
plutil -convert json -o "$out/daemon.json" "$plist"
plutil -convert json -o "$out/agent.json" "/Library/LaunchAgents/$agent.plist"
python3 - "$out" "$image" <<'PY'
import json,pathlib,sys
p=pathlib.Path(sys.argv[1]); image=sys.argv[2]
assert json.loads((p/'daemon.json').read_text())['ProgramArguments']==[image,'--capture-broker']
assert json.loads((p/'agent.json').read_text())['ProgramArguments']==[image,'--user-runtime-login']
PY
# Run the real native host in the same signed immutable image as the root
# broker. Stop the login job first so the harness is the sole forwarding owner.
if /bin/launchctl print "gui/$uid/$agent" > "$out/user-job-before-probe.txt" 2>&1; then
  sudo /bin/launchctl bootout "gui/$uid/$agent"
fi
"$image" --capture-acceptance > "$out/native-probes.jsonl" 2> "$out/native-probes.stderr"
route -n get default > "$out/default-detached.txt"
netstat -rn > "$out/routes-detached.txt"
scutil --dns > "$out/dns-detached.txt"
ifconfig -l > "$out/interfaces-detached.txt"
cleanup
trap - EXIT
set -e
[[ ! -e "$out/cleanup-failed.txt" ]] || exit 1
python3 tools/desktop/check_macos_capture_acceptance.py "$out"
echo 'native capture acceptance passed'
