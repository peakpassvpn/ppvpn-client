#!/bin/sh
# Generates the lab's keys, certificates, server configurations and profile
# into /work. Runs inside the saillab image with sail at /sail/sail.
set -eu
cd /work
NET=${NET:-198.51.100}
if [ ! -f ca.crt ]; then
  openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 30 -subj "/CN=sail-lab-ca" -keyout ca.key -out ca.crt 2>/dev/null
  cert() { # name, SAN
    openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -subj "/CN=$1" -keyout $1.key -out $1.csr 2>/dev/null
    printf "subjectAltName=%s\n" "$2" > $1.ext
    openssl x509 -req -in $1.csr -CA ca.crt -CAkey ca.key -CAcreateserial -days 30 -extfile $1.ext -out $1.crt 2>/dev/null
  }
  cert web "DNS:www.lab.test,DNS:*.lab.test,DNS:*.video.lab.test,IP:$NET.50"
  cert us "DNS:us.lab.test"
  cert dot "IP:1.1.1.1,IP:8.8.8.8,IP:9.9.9.9,DNS:one.one.one.one,DNS:dns.google,DNS:dns.quad9.net"
  /sail/sail generate reality-keypair > reality.txt
  /sail/sail generate ss2022 2022-blake3-aes-128-gcm > ss-server.key
  /sail/sail generate ss2022 2022-blake3-aes-128-gcm > ss-user.key
  /sail/sail generate uuid > uuid.txt
fi
PRIV=$(sed -n 's/^PrivateKey: //p' reality.txt); PUB=$(sed -n 's/^PublicKey: //p' reality.txt)
SSK=$(cat ss-server.key); SSU=$(cat ss-user.key); UUID=$(cat uuid.txt)

# Node servers: Sail itself, one protocol each, all leaving directly.
cat > srv-a.json <<J
{"log":{"level":"info","timestamp":true},
 "inbounds":[{"type":"vless","tag":"in","listen":"0.0.0.0","listen_port":443,
   "users":[{"name":"u","uuid":"$UUID","flow":"xtls-rprx-vision"}],
   "tls":{"enabled":true,"server_name":"www.lab.test","reality":{"enabled":true,"handshake":{"server":"$NET.50","server_port":443},"private_key":"$PRIV","short_id":["01"]}}}],
 "outbounds":[{"type":"direct","tag":"direct"}]}
J
cat > srv-b.json <<J
{"log":{"level":"info","timestamp":true},
 "inbounds":[{"type":"shadowsocks","tag":"in","listen":"0.0.0.0","listen_port":8443,"method":"2022-blake3-aes-128-gcm","password":"$SSK",
   "users":[{"name":"u","password":"$SSU"}]}],
 "outbounds":[{"type":"direct","tag":"direct"}]}
J
cat > srv-c.json <<J
{"log":{"level":"info","timestamp":true},
 "inbounds":[{"type":"anytls","tag":"in","listen":"0.0.0.0","listen_port":8444,"users":[{"name":"u","password":"anytls-lab-password"}],
   "tls":{"enabled":true,"server_name":"us.lab.test","certificate_path":"/work/us.crt","key_path":"/work/us.key"}}],
 "outbounds":[{"type":"direct","tag":"direct"}]}
J
for f in srv-a srv-b srv-c; do /sail/sail -c $f.json -T >/dev/null || { echo "$f invalid"; /sail/sail -c $f.json -T; exit 1; }; done

# The profile: JP = VLESS/REALITY primary (srv-a) + SS2022 backup (srv-b);
# US = AnyTLS (srv-c). Ingresses carry only a domain (as from the backend
# when no entry IP is pinned): the lab's node DNS resolves them to the
# TEST-NET-2 addresses, which core's validation would refuse as entry IPs.
cat > profile.json <<J
{"schema_version":1,"revision":"lab#${REV:-1}","generated_at":"2026-10-01T00:00:00Z","expires_at":"2099-01-01T00:00:00Z",
 "nodes":[
  {"id":"jp","name":"JP","entry_key":"e","exit":{"ip":"$NET.11","region":"JP"},"capabilities":{"tcp":true,"udp":true},
   "ingresses":[
    {"role":"primary","endpoint_key":"9001","replica_ordinal":0,"protocol":"vless",
     "endpoint":{"domain":"jp-a.lab.test","port":443},
     "credentials":{"vless":{"uuid":"$UUID","flow":"xtls-rprx-vision"}},
     "tls":{"server_name":"www.lab.test","reality":{"public_key":"$PUB","short_id":"01"}},
     "capabilities":{"tcp":true,"udp":true}},
    {"role":"backup","endpoint_key":"9002","label":"JP relay","replica_ordinal":1,"protocol":"shadowsocks",
     "endpoint":{"domain":"jp-b.lab.test","port":8443},
     "credentials":{"shadowsocks":{"method":"2022-blake3-aes-128-gcm","user_key":"$SSU","identity_keys":["$SSK"]}},
     "capabilities":{"tcp":true,"udp":true}}]},
  {"id":"us","name":"US","entry_key":"e","exit":{"region":"US"},"capabilities":{"tcp":true,"udp":false},
   "ingresses":[
    {"role":"primary","endpoint_key":"9003","replica_ordinal":0,"protocol":"anytls",
     "endpoint":{"domain":"us.lab.test","port":8444},
     "credentials":{"anytls":{"password":"anytls-lab-password"}},
     "tls":{"server_name":"us.lab.test"},
     "capabilities":{"tcp":true,"udp":false}}]}],
 "selection":{"mode":"manual","default_node_id":"jp"},
 "routing":{"rules":[
   {"id":"video","match":{"domain_suffixes":["video.lab.test"]},"action":{"type":"proxy","target":"node","node_id":"us"}}],
  "final":{"type":"proxy","target":"selected"}}}
J
echo setup-ok
