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
for name in ['ppvpn-service','ppvpn-service-install','ppvpn-service-uninstall']:
    assert hashlib.sha256((p/name).read_bytes()).hexdigest()==m['sha256'][name], name
PY
team="$(tr -d '\n' < crates/service/src/macos_team_id)"
for name in ppvpn-service ppvpn-service-install ppvpn-service-uninstall; do
  id="com.peakpassvpn.ppvpn.${name#ppvpn-}"
  codesign --verify --strict --verbose=2 -R "anchor apple generic and identifier \"$id\" and certificate 1[field.1.2.840.113635.100.6.2.6] and certificate leaf[field.1.2.840.113635.100.6.1.13] and certificate leaf[subject.OU] = \"$team\"" "$artifact/$name"
done
"$artifact/ppvpn-service" --build-info > "$out/build-info.json"
python3 - "$out/build-info.json" <<'PY'
import json,sys
m=json.load(open(sys.argv[1])); assert m['user_runtime'] and m['capture_acceptance']
PY
netstat -rn > "$out/routes-before.txt"
scutil --dns > "$out/dns-before.txt"
route -n get default > "$out/default-before.txt"
installed=0
cleanup() {
  set +e
  if (( installed )); then
    sudo /bin/launchctl bootout "gui/$uid/$agent" 2>/dev/null
    sudo "$artifact/ppvpn-service-uninstall" > "$out/uninstall.txt" 2>&1
  fi
  sudo rm -f "$marker"
  netstat -rn > "$out/routes-after.txt"
  scutil --dns > "$out/dns-after.txt"
  route -n get default > "$out/default-after.txt"
  [[ ! -e /private/var/run/com.peakpassvpn.capture/grant.json ]] || echo BROKER_JOURNAL_LEFTOVER > "$out/cleanup-failed.txt" # sensitive-check: allow — canonical macOS runtime journal
  [[ ! -e "$image" && ! -e "$plist" && ! -e "/Library/LaunchAgents/$agent.plist" ]] || echo INSTALLATION_LEFTOVER >> "$out/cleanup-failed.txt"
}
trap cleanup EXIT
sudo /usr/bin/install -o root -g wheel -m 644 /dev/null "$marker"
installed=1
sudo "$artifact/ppvpn-service-install" > "$out/install.txt" 2>&1
sudo /bin/launchctl print "system/$daemon" > "$out/root-job.txt"
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
python3 - "$out" <<'PY'
import json,pathlib,sys
p=pathlib.Path(sys.argv[1]); rows=[json.loads(x) for x in (p/'native-probes.jsonl').read_text().splitlines() if x.startswith('{')]
assert len([x for x in rows if x.get('probe')=='attached'])==10
assert rows[-1]['probe']=='complete'
def route(file):
    return [x.strip() for x in file.read_text().splitlines() if x.strip().startswith(('gateway:','interface:','destination:','mask:'))]
assert route(p/'default-before.txt')==route(p/'default-detached.txt'), 'default route changed'
PY
cleanup
trap - EXIT
[[ ! -e "$out/cleanup-failed.txt" ]] || exit 1
echo 'native capture acceptance passed'
