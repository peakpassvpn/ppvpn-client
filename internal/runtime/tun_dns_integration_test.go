package runtime

import (
	"context"
	"crypto/tls"
	"encoding/binary"
	"errors"
	"io"
	"net"
	"net/netip"
	"os"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/miekg/dns"
	"github.com/peakpassvpn/ppvpn-core/internal/config"
	"github.com/peakpassvpn/ppvpn-core/internal/corelog"
	"github.com/peakpassvpn/ppvpn-core/internal/dnstransport"
	"github.com/peakpassvpn/ppvpn-core/internal/failover"
	"github.com/peakpassvpn/ppvpn-core/profile"
	box "github.com/sagernet/sing-box"
	"github.com/sagernet/sing-box/adapter"
	singtls "github.com/sagernet/sing-box/common/tls"
	C "github.com/sagernet/sing-box/constant"
	"github.com/sagernet/sing-box/option"
	"github.com/sagernet/sing/common/json/badoption"
	M "github.com/sagernet/sing/common/metadata"
	N "github.com/sagernet/sing/common/network"
	"github.com/sagernet/sing/protocol/socks"
)

// destinationRecorder is a tracker on the stand-in Shadowsocks server: it
// records the destination each client connection asked the node for.
type destinationRecorder struct {
	mu   sync.Mutex
	seen []string
}

func (r *destinationRecorder) RoutedConnection(_ context.Context, conn net.Conn, metadata adapter.InboundContext, _ adapter.Rule, _ adapter.Outbound) net.Conn {
	r.mu.Lock()
	r.seen = append(r.seen, metadata.Destination.String())
	r.mu.Unlock()
	return conn
}

func (r *destinationRecorder) RoutedPacketConnection(_ context.Context, conn N.PacketConn, metadata adapter.InboundContext, _ adapter.Rule, _ adapter.Outbound) N.PacketConn {
	r.mu.Lock()
	r.seen = append(r.seen, "udp:"+metadata.Destination.String())
	r.mu.Unlock()
	return conn
}

func (r *destinationRecorder) reset() {
	r.mu.Lock()
	r.seen = nil
	r.mu.Unlock()
}

func (r *destinationRecorder) has(destination string) bool {
	r.mu.Lock()
	defer r.mu.Unlock()
	return slices.Contains(r.seen, destination)
}

func (r *destinationRecorder) wait(t *testing.T, destination string) {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for !r.has(destination) {
		if time.Now().After(deadline) {
			r.mu.Lock()
			defer r.mu.Unlock()
			t.Fatalf("node never asked for %s; saw %v", destination, r.seen)
		}
		time.Sleep(20 * time.Millisecond)
	}
}

type writerFunc func([]byte) (int, error)

func (f writerFunc) Write(p []byte) (int, error) { return f(p) }

// startFakeDNS answers A queries over TCP from a fixed table.
func startFakeDNS(t *testing.T, answers map[string]string) uint16 {
	t.Helper()
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	server := &dns.Server{Listener: listener, Handler: dns.HandlerFunc(func(w dns.ResponseWriter, request *dns.Msg) {
		response := new(dns.Msg)
		response.SetReply(request)
		question := request.Question[0]
		if address, ok := answers[question.Name]; !ok {
			response.Rcode = dns.RcodeNameError
		} else if question.Qtype == dns.TypeA {
			response.Answer = append(response.Answer, &dns.A{Hdr: dns.RR_Header{Name: question.Name, Rrtype: dns.TypeA, Class: dns.ClassINET, Ttl: 60}, A: net.ParseIP(address)})
		}
		_ = w.WriteMsg(response)
	})}
	go func() { _ = server.ActivateAndServe() }()
	t.Cleanup(func() { _ = server.Shutdown() })
	return uint16(listener.Addr().(*net.TCPAddr).Port)
}

// socksConnect sends a SOCKS5 CONNECT for an IPv4 or IPv6 destination, the
// way a TUN connection reaches the router: an address and no domain. sing-box
// answers the CONNECT lazily, once routing reads from the connection.
func socksConnect(t *testing.T, port uint16, destination netip.AddrPort) net.Conn {
	t.Helper()
	conn, err := net.DialTimeout("tcp", net.JoinHostPort("127.0.0.1", strconv.Itoa(int(port))), time.Second)
	if err != nil {
		t.Fatal(err)
	}
	_ = conn.SetDeadline(time.Now().Add(5 * time.Second))
	request := []byte{5, 1, 0, 5, 1, 0}
	if destination.Addr().Is4() {
		ip := destination.Addr().As4()
		request = append(append(request, 1), ip[:]...)
	} else {
		ip := destination.Addr().As16()
		request = append(append(request, 4), ip[:]...)
	}
	request = binary.BigEndian.AppendUint16(request, destination.Port())
	if _, err = conn.Write(request); err != nil {
		t.Fatal(err)
	}
	method := make([]byte, 2)
	if _, err = io.ReadFull(conn, method); err != nil || method[1] != 0 {
		t.Fatalf("socks method: %v %v", method, err)
	}
	reply := make([]byte, 4)
	if _, err = io.ReadFull(conn, reply); err != nil || reply[1] != 0 {
		t.Fatalf("socks connect: %v %v", reply, err)
	}
	bound := 4 + 2
	if reply[3] == 4 {
		bound = 16 + 2
	}
	if _, err = io.ReadFull(conn, make([]byte, bound)); err != nil {
		t.Fatalf("socks bound address: %v", err)
	}
	return conn
}

// TestTUNRouteResolvesAndHandsDomainsToNode runs the TUN route and DNS
// configuration on a real sing-box. The TUN inbound needs privileges, so a
// SOCKS inbound carrying the TUN tag feeds IP destinations into the same
// rules. The remote DNS server is swapped for a local fake over plain TCP
// (instead of DoT to 1.1.1.1), still dialed through the selected node.
func TestTUNRouteResolvesAndHandsDomainsToNode(t *testing.T) {
	recorder := &destinationRecorder{}
	serverPort := startShadowsocksServerWithTracker(t, recorder)
	dnsPort := startFakeDNS(t, map[string]string{"proxied.test.": "203.0.113.7"})

	p := &profile.Profile{
		SchemaVersion: profile.CurrentSchemaVersion, Revision: "tun-dns", ExpiresAt: time.Now().Add(time.Hour),
		Nodes: []profile.Node{{ID: "node", EntryKey: "cn-optimized", Capabilities: profile.Capabilities{TCP: true}, Ingresses: []profile.Ingress{
			localSSIngress(profile.IngressRolePrimary, "n0", 0, serverPort),
		}}},
		Selection: profile.Selection{Mode: "manual", DefaultNodeID: "node"},
		Routing:   profile.Routing{Final: profile.RoutingAction{Type: "proxy", Target: "selected"}},
	}
	built, err := config.Build(p, profile.PlatformCapabilities{Platform: "linux", TUN: profile.TUNCapabilities{Enabled: true}, LogLevel: "error"}, time.Now())
	if err != nil {
		t.Fatal(err)
	}
	options := built.Options
	socksPort := freePort(t)
	listen := badoption.Addr(netip.MustParseAddr("127.0.0.1"))
	options.Inbounds = []option.Inbound{{Type: C.TypeSOCKS, Tag: config.TUNInboundTag, Options: &option.SocksInboundOptions{ListenOptions: option.ListenOptions{Listen: &listen, ListenPort: socksPort}}}}
	options.Route.AutoDetectInterface = false
	replaced := false
	for i, server := range options.DNS.Servers {
		if server.Tag != config.DNSRemoteTag {
			continue
		}
		detour := server.Options.(*option.RemoteTLSDNSServerOptions).Detour
		options.DNS.Servers[i] = option.DNSServerOptions{Type: C.DNSTypeTCP, Tag: config.DNSRemoteTag, Options: &option.RemoteDNSServerOptions{
			RawLocalDNSServerOptions: option.RawLocalDNSServerOptions{DialerOptions: option.DialerOptions{Detour: detour}},
			DNSServerAddressOptions:  option.DNSServerAddressOptions{Server: "127.0.0.1", ServerPort: dnsPort},
		}}
		replaced = detour == "selected"
	}
	if !replaced {
		t.Fatal("remote DNS server is not dialed through the selected node")
	}
	var debugLog strings.Builder
	var debugMu sync.Mutex
	connectionLog := corelog.New(writerFunc(func(p []byte) (int, error) {
		debugMu.Lock()
		defer debugMu.Unlock()
		return debugLog.Write(p)
	}))
	_ = connectionLog.SetLevel(corelog.LevelDebug)
	ctx, cancel := context.WithCancel(dnstransport.WithLogger(context.Background(), connectionLog))
	defer cancel()
	instance, err := box.New(box.Options{Context: failover.Context(ctx), Options: options})
	if err != nil {
		t.Fatal(err)
	}
	tracker := newTelemetry()
	tracker.log.Store(connectionLog)
	instance.Router().AppendTracker(tracker)
	if err = instance.Start(); err != nil {
		t.Fatal(err)
	}
	defer instance.Close()

	// 1. DNS sent to any address on port 53 is hijacked into the DNS module
	// and, for a proxied name, resolved through the node.
	conn := socksConnect(t, socksPort, netip.MustParseAddrPort("10.255.0.1:53"))
	query := new(dns.Msg)
	query.SetQuestion("proxied.test.", dns.TypeA)
	packed, _ := query.Pack()
	if _, err = conn.Write(append(binary.BigEndian.AppendUint16(nil, uint16(len(packed))), packed...)); err != nil {
		t.Fatal(err)
	}
	var length uint16
	if err = binary.Read(conn, binary.BigEndian, &length); err != nil {
		t.Fatalf("hijacked DNS: %v", err)
	}
	raw := make([]byte, length)
	if _, err = io.ReadFull(conn, raw); err != nil {
		t.Fatal(err)
	}
	conn.Close()
	var answer dns.Msg
	if err = answer.Unpack(raw); err != nil || len(answer.Answer) != 1 || answer.Answer[0].(*dns.A).A.String() != "203.0.113.7" {
		t.Fatalf("answer: %v %v", err, answer.Answer)
	}
	recorder.wait(t, net.JoinHostPort("127.0.0.1", strconv.Itoa(int(dnsPort))))
	// At debug level the upstream exchange is logged with its server and rcode.
	debugMu.Lock()
	dnsLogged := debugLog.String()
	debugMu.Unlock()
	if !strings.Contains(dnsLogged, "msg=dns name=proxied.test. type=A server=dns-remote attempt=1 rcode=NOERROR answers=1 ms=") {
		t.Fatalf("debug dns line:\n%s", dnsLogged)
	}

	// 2. A connection to the address DNS answered carries the domain to the
	// node, even though the sniffed HTTP Host is only the address.
	conn = socksConnect(t, socksPort, netip.MustParseAddrPort("203.0.113.7:80"))
	_, _ = conn.Write([]byte("GET / HTTP/1.1\r\nHost: 203.0.113.7\r\n\r\n"))
	recorder.wait(t, "proxied.test:80")
	// At debug level the connection line says the node got the domain, even
	// though the HTTP sniffer saw only the address as Host (route_domain):
	// domaindest fell back to the DNS reverse mapping.
	debugMu.Lock()
	logged := debugLog.String()
	debugMu.Unlock()
	if !strings.Contains(logged, "destination=203.0.113.7:80 route_domain=203.0.113.7 protocol=http") || !strings.Contains(logged, "target=proxied.test:80 target_kind=domain") {
		t.Fatalf("debug connection line:\n%s", logged)
	}
	conn.Close()

	// 3. A sniffed TLS server name reaches the node instead of the address.
	conn = socksConnect(t, socksPort, netip.MustParseAddrPort("203.0.113.8:443"))
	go func() {
		_ = tls.Client(conn, &tls.Config{ServerName: "sni.test", InsecureSkipVerify: true}).Handshake()
	}()
	recorder.wait(t, "sni.test:443")
	conn.Close()

	// 4. A fake-ip address with a known domain is proxied by name ...
	conn = socksConnect(t, socksPort, netip.MustParseAddrPort("198.18.1.30:443"))
	go func() {
		_ = tls.Client(conn, &tls.Config{ServerName: "fake.test", InsecureSkipVerify: true}).Handshake()
	}()
	recorder.wait(t, "fake.test:443")
	conn.Close()

	// 5. ... but without one it is closed at once and never reaches the node.
	started := time.Now()
	conn = socksConnect(t, socksPort, netip.MustParseAddrPort("198.18.1.29:443"))
	_, err = io.Copy(io.Discard, conn)
	elapsed := time.Since(started)
	conn.Close()
	var netErr net.Error
	if errors.As(err, &netErr) && netErr.Timeout() {
		t.Fatal("fake-ip connection without a domain was left open")
	}
	if elapsed > 2*time.Second {
		t.Fatalf("fake-ip connection took %s to fail", elapsed)
	}
	time.Sleep(200 * time.Millisecond)
	if recorder.has("198.18.1.29:443") {
		t.Fatal("fake-ip address without a domain was sent to the node")
	}

	// 5b. Plain HTTP to a fake-ip address sniffs its Host header, which is
	// the IP literal itself: that is not a domain, so it is rejected as well.
	started = time.Now()
	conn = socksConnect(t, socksPort, netip.MustParseAddrPort("198.18.1.28:80"))
	_, _ = conn.Write([]byte("GET / HTTP/1.1\r\nHost: 198.18.1.28\r\nUser-Agent: curl/8.0\r\nAccept: */*\r\n\r\n"))
	_, err = io.Copy(io.Discard, conn)
	elapsed = time.Since(started)
	conn.Close()
	if errors.As(err, &netErr) && netErr.Timeout() {
		t.Fatal("fake-ip HTTP connection with an IP Host was left open")
	}
	if elapsed > 2*time.Second {
		t.Fatalf("fake-ip HTTP connection with an IP Host took %s to fail", elapsed)
	}
	time.Sleep(200 * time.Millisecond)
	if recorder.has("198.18.1.28:80") {
		t.Fatal("fake-ip address with an IP-literal Host was sent to the node")
	}

	// 5c. Non-DNS traffic to the tunnel's own peer address exists nowhere:
	// it is rejected at once (#17), and never reaches the node.
	started = time.Now()
	conn = socksConnect(t, socksPort, netip.MustParseAddrPort("10.60.159.90:8080"))
	_, err = io.Copy(io.Discard, conn)
	elapsed = time.Since(started)
	conn.Close()
	if errors.As(err, &netErr) && netErr.Timeout() || elapsed > 2*time.Second {
		t.Fatalf("tunnel peer connection lingered %s: %v", elapsed, err)
	}

	// 5d. The client baseline keeps LAN and multicast destinations off the
	// node whatever the profile says (this profile has no private rule).
	for _, destination := range []string{"239.255.255.250:1900", "192.168.1.10:8080", "100.100.100.100:443"} {
		conn = socksConnect(t, socksPort, netip.MustParseAddrPort(destination))
		_, _ = conn.Write([]byte{0})
		conn.Close()
	}
	time.Sleep(300 * time.Millisecond)
	for _, destination := range []string{"10.60.159.90:8080", "239.255.255.250:1900", "192.168.1.10:8080", "100.100.100.100:443"} {
		if recorder.has(destination) {
			t.Fatalf("%s was sent to the node", destination)
		}
	}

	// 6. DNS to an IPv6 resolver is hijacked the same way (desktop TUN routes
	// IPv6 into the tunnel, so an ISP's IPv6 DNS must not escape).
	conn = socksConnect(t, socksPort, netip.MustParseAddrPort("[2001:db8::53]:53"))
	if _, err = conn.Write(append(binary.BigEndian.AppendUint16(nil, uint16(len(packed))), packed...)); err != nil {
		t.Fatal(err)
	}
	if err = binary.Read(conn, binary.BigEndian, &length); err != nil {
		t.Fatalf("hijacked IPv6 DNS: %v", err)
	}
	raw = make([]byte, length)
	if _, err = io.ReadFull(conn, raw); err != nil {
		t.Fatal(err)
	}
	conn.Close()
	answer = dns.Msg{}
	if err = answer.Unpack(raw); err != nil || len(answer.Answer) != 1 || answer.Answer[0].(*dns.A).A.String() != "203.0.113.7" {
		t.Fatalf("IPv6 resolver answer: %v %v", err, answer.Answer)
	}
	if recorder.has("[2001:db8::53]:53") {
		t.Fatal("DNS to an IPv6 resolver reached the node instead of being hijacked")
	}

	// 7. An IPv6 literal destination without a known domain is proxied to
	// the node as the address.
	conn = socksConnect(t, socksPort, netip.MustParseAddrPort("[2001:db8::7]:8443"))
	_, _ = conn.Write([]byte{0x00, 0x01, 0x02, 0x03})
	recorder.wait(t, "[2001:db8::7]:8443")
	conn.Close()
}

// TestTUNRuleSetDomainGoesDirect runs a local binary rule set on a real
// sing-box with the TUN route (fed by a SOCKS inbound carrying the TUN tag):
// a sniffed domain in a direct rule set connects straight to its address,
// every other domain goes to the node.
func TestTUNRuleSetDomainGoesDirect(t *testing.T) {
	recorder := &destinationRecorder{}
	serverPort := startShadowsocksServerWithTracker(t, recorder)
	target, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer target.Close()
	accepted := make(chan struct{}, 4)
	go func() {
		for {
			conn, err := target.Accept()
			if err != nil {
				return
			}
			accepted <- struct{}{}
			conn.Close()
		}
	}()

	body := domainRuleSet(t, "direct.test")
	path := filepath.Join(t.TempDir(), "direct-sites.srs")
	if err = os.WriteFile(path, body, 0o600); err != nil {
		t.Fatal(err)
	}
	p := &profile.Profile{
		SchemaVersion: profile.CurrentSchemaVersion, Revision: "tun-rule-set", ExpiresAt: time.Now().Add(time.Hour),
		Nodes: []profile.Node{{ID: "node", EntryKey: "cn-optimized", Capabilities: profile.Capabilities{TCP: true}, Ingresses: []profile.Ingress{
			localSSIngress(profile.IngressRolePrimary, "n0", 0, serverPort),
		}}},
		Selection: profile.Selection{Mode: "manual", DefaultNodeID: "node"},
		Routing: profile.Routing{
			RuleSets: []profile.RuleSet{{ID: "direct-sites", URL: "https://api.example.com/direct-sites.srs", SHA256: sha256Hex(body)}},
			Rules:    []profile.RoutingRule{{ID: "direct-sites", Match: profile.RoutingMatch{RuleSetIDs: []string{"direct-sites"}}, Action: profile.RoutingAction{Type: "direct"}}},
			Final:    profile.RoutingAction{Type: "proxy", Target: "selected"},
		},
	}
	built, err := config.BuildWithOptions(p, profile.PlatformCapabilities{Platform: "linux", TUN: profile.TUNCapabilities{Enabled: true}, LogLevel: "error"},
		config.BuildOptions{RuleSets: map[string]config.RuleSetFile{"direct-sites": {Path: path, MirrorDNS: true}}}, time.Now())
	if err != nil {
		t.Fatal(err)
	}
	options := built.Options
	socksPort := freePort(t)
	listen := badoption.Addr(netip.MustParseAddr("127.0.0.1"))
	options.Inbounds = []option.Inbound{{Type: C.TypeSOCKS, Tag: config.TUNInboundTag, Options: &option.SocksInboundOptions{ListenOptions: option.ListenOptions{Listen: &listen, ListenPort: socksPort}}}}
	options.Route.AutoDetectInterface = false
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	instance, err := box.New(box.Options{Context: failover.Context(ctx), Options: options})
	if err != nil {
		t.Fatal(err)
	}
	if err = instance.Start(); err != nil {
		t.Fatal(err)
	}
	defer instance.Close()

	// A domain the rule set matches is dialed directly at its address.
	targetAddress := netip.MustParseAddrPort(target.Addr().String())
	conn := socksConnect(t, socksPort, targetAddress)
	go func() {
		_ = tls.Client(conn, &tls.Config{ServerName: "www.direct.test", InsecureSkipVerify: true}).Handshake()
	}()
	select {
	case <-accepted:
	case <-time.After(5 * time.Second):
		t.Fatal("rule set domain was not dialed directly")
	}
	conn.Close()

	// Any other domain goes to the node.
	conn = socksConnect(t, socksPort, netip.MustParseAddrPort("203.0.113.8:443"))
	go func() {
		_ = tls.Client(conn, &tls.Config{ServerName: "elsewhere.test", InsecureSkipVerify: true}).Handshake()
	}()
	recorder.wait(t, "elsewhere.test:443")
	conn.Close()
	if recorder.has("www.direct.test:443") {
		t.Fatal("rule set domain was sent to the node")
	}
}

// startFakeDoT is startFakeDNS over TLS (a fresh self-signed certificate;
// clients skip verification).
func startFakeDoT(t *testing.T, answers map[string]string) uint16 {
	t.Helper()
	certificate, err := singtls.GenerateKeyPair(nil, nil, time.Now, "dns.test")
	if err != nil {
		t.Fatal(err)
	}
	listener, err := tls.Listen("tcp", "127.0.0.1:0", &tls.Config{Certificates: []tls.Certificate{*certificate}})
	if err != nil {
		t.Fatal(err)
	}
	server := &dns.Server{Listener: listener, Net: "tcp-tls", Handler: dns.HandlerFunc(func(w dns.ResponseWriter, request *dns.Msg) {
		response := new(dns.Msg)
		response.SetReply(request)
		question := request.Question[0]
		if address, ok := answers[question.Name]; !ok {
			response.Rcode = dns.RcodeNameError
		} else if question.Qtype == dns.TypeA {
			response.Answer = append(response.Answer, &dns.A{Hdr: dns.RR_Header{Name: question.Name, Rrtype: dns.TypeA, Class: dns.ClassINET, Ttl: 60}, A: net.ParseIP(address)})
		}
		_ = w.WriteMsg(response)
	})}
	go func() { _ = server.ActivateAndServe() }()
	t.Cleanup(func() { _ = server.Shutdown() })
	return uint16(listener.Addr().(*net.TCPAddr).Port)
}

// startRemoteDNSBox runs the TUN config behind a SOCKS stand-in for the TUN
// inbound, through a Shadowsocks node, with the remote DoT servers
// (dns-remote and its fallbacks) pointed at 127.0.0.1:ports[tag] and
// certificate verification off. Every remote server must be given. It returns
// the SOCKS port and a reader of the debug log.
func startRemoteDNSBox(t *testing.T, ports map[string]uint16) (uint16, func() string) {
	t.Helper()
	serverPort := startShadowsocksServerWithTracker(t, &destinationRecorder{})
	p := &profile.Profile{
		SchemaVersion: profile.CurrentSchemaVersion, Revision: "tun-dns-remote", ExpiresAt: time.Now().Add(time.Hour),
		Nodes: []profile.Node{{ID: "node", EntryKey: "cn-optimized", Capabilities: profile.Capabilities{TCP: true}, Ingresses: []profile.Ingress{
			localSSIngress(profile.IngressRolePrimary, "n0", 0, serverPort),
		}}},
		Selection: profile.Selection{Mode: "manual", DefaultNodeID: "node"},
		Routing:   profile.Routing{Final: profile.RoutingAction{Type: "proxy", Target: "selected"}},
	}
	built, err := config.Build(p, profile.PlatformCapabilities{Platform: "linux", TUN: profile.TUNCapabilities{Enabled: true}, LogLevel: "error"}, time.Now())
	if err != nil {
		t.Fatal(err)
	}
	options := built.Options
	socksPort := freePort(t)
	listen := badoption.Addr(netip.MustParseAddr("127.0.0.1"))
	options.Inbounds = []option.Inbound{{Type: C.TypeSOCKS, Tag: config.TUNInboundTag, Options: &option.SocksInboundOptions{ListenOptions: option.ListenOptions{Listen: &listen, ListenPort: socksPort}}}}
	options.Route.AutoDetectInterface = false
	remaining := len(ports)
	for _, server := range options.DNS.Servers {
		tlsOptions, ok := server.Options.(*option.RemoteTLSDNSServerOptions)
		if !ok {
			continue
		}
		port, ok := ports[server.Tag]
		if !ok {
			t.Fatalf("no port for %s", server.Tag)
		}
		if server.Type != C.DNSTypeTLS || tlsOptions.Detour != "selected" {
			t.Fatalf("%s is not DoT through the selected node", server.Tag)
		}
		tlsOptions.Server, tlsOptions.ServerPort = "127.0.0.1", port
		tlsOptions.TLS = &option.OutboundTLSOptions{Enabled: true, ServerName: "dns.test", Insecure: true}
		remaining--
	}
	if remaining != 0 {
		t.Fatalf("remote servers %v, config has %d fewer", ports, remaining)
	}
	var debugLog strings.Builder
	var debugMu sync.Mutex
	logger := corelog.New(writerFunc(func(p []byte) (int, error) {
		debugMu.Lock()
		defer debugMu.Unlock()
		return debugLog.Write(p)
	}))
	_ = logger.SetLevel(corelog.LevelDebug)
	ctx, cancel := context.WithCancel(dnstransport.WithLogger(context.Background(), logger))
	t.Cleanup(cancel)
	instance, err := box.New(box.Options{Context: failover.Context(ctx), Options: options})
	if err != nil {
		t.Fatal(err)
	}
	if err = instance.Start(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = instance.Close() })
	return socksPort, func() string {
		debugMu.Lock()
		defer debugMu.Unlock()
		return debugLog.String()
	}
}

// resolveTCP sends a hijacked DNS query over TCP (as a TUN client would to
// port 53) and returns the response and how long it took.
func resolveTCP(t *testing.T, socksPort uint16, name string) (*dns.Msg, time.Duration) {
	t.Helper()
	started := time.Now()
	conn := socksConnect(t, socksPort, netip.MustParseAddrPort("10.255.0.1:53"))
	defer conn.Close()
	query := new(dns.Msg)
	query.SetQuestion(name, dns.TypeA)
	packed, _ := query.Pack()
	if _, err := conn.Write(append(binary.BigEndian.AppendUint16(nil, uint16(len(packed))), packed...)); err != nil {
		t.Fatal(err)
	}
	_ = conn.SetReadDeadline(time.Now().Add(15 * time.Second))
	var length uint16
	if err := binary.Read(conn, binary.BigEndian, &length); err != nil {
		t.Fatalf("hijacked DNS over TCP after %s: %v", time.Since(started), err)
	}
	raw := make([]byte, length)
	if _, err := io.ReadFull(conn, raw); err != nil {
		t.Fatal(err)
	}
	response := new(dns.Msg)
	if err := response.Unpack(raw); err != nil {
		t.Fatal(err)
	}
	return response, time.Since(started)
}

// resolveUDP is resolveTCP over UDP, through a SOCKS5 UDP association.
func resolveUDP(t *testing.T, socksPort uint16, name string) (*dns.Msg, time.Duration) {
	t.Helper()
	started := time.Now()
	client := socks.NewClient(N.SystemDialer, M.ParseSocksaddrHostPort("127.0.0.1", socksPort), socks.Version5, "", "")
	destination := M.ParseSocksaddr("10.255.0.1:53")
	conn, err := client.ListenPacket(context.Background(), destination)
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()
	query := new(dns.Msg)
	query.SetQuestion(name, dns.TypeA)
	packed, _ := query.Pack()
	if _, err = conn.WriteTo(packed, destination.UDPAddr()); err != nil {
		t.Fatal(err)
	}
	_ = conn.SetReadDeadline(time.Now().Add(15 * time.Second))
	raw := make([]byte, 4096)
	n, _, err := conn.ReadFrom(raw)
	if err != nil {
		t.Fatalf("hijacked DNS over UDP after %s: %v", time.Since(started), err)
	}
	response := new(dns.Msg)
	if err = response.Unpack(raw[:n]); err != nil {
		t.Fatal(err)
	}
	return response, time.Since(started)
}

// When dns-remote fails through the node, the guard falls back to the next
// DoT server in a real sing-box and gets the answer on attempt 2; the next
// uncached query starts at that server.
func TestTUNRemoteDNSFallsBackThroughTheNode(t *testing.T) {
	dotPort := startFakeDoT(t, map[string]string{"proxied.test.": "203.0.113.7", "again.proxied.test.": "203.0.113.7"})
	socksPort, logged := startRemoteDNSBox(t, map[string]uint16{
		config.DNSRemoteTag:             freePort(t), // closed
		config.DNSRemoteFallbackTags[0]: dotPort,
		config.DNSRemoteFallbackTags[1]: freePort(t), // closed, never reached
	})
	answered := func(name string) {
		t.Helper()
		response, _ := resolveTCP(t, socksPort, name)
		if response.Rcode != dns.RcodeSuccess || len(response.Answer) != 1 || response.Answer[0].(*dns.A).A.String() != "203.0.113.7" {
			t.Fatalf("%s: %v", name, response)
		}
	}

	answered("proxied.test.")
	first := logged()
	if !strings.Contains(first, "msg=dns name=proxied.test. type=A server=dns-remote attempt=1 error=") ||
		!strings.Contains(first, "msg=dns name=proxied.test. type=A server=dns-remote-8.8.8.8 attempt=2 rcode=NOERROR answers=1") {
		t.Fatalf("fallback lines:\n%s", first)
	}
	// Every fallback server exists in a real sing-box built from the config.
	if strings.Contains(first, "fallback missing") {
		t.Fatalf("fallback missing:\n%s", first)
	}
	// The next uncached query starts at the upstream that answered.
	answered("again.proxied.test.")
	second := strings.TrimPrefix(logged(), first)
	if !strings.Contains(second, "msg=dns name=again.proxied.test. type=A server=dns-remote-8.8.8.8 attempt=1 rcode=NOERROR answers=1") ||
		strings.Contains(second, "server=dns-remote attempt") {
		t.Fatalf("preferred upstream lines:\n%s", second)
	}
}

// When every remote server fails, a hijacked query is answered SERVFAIL over
// TCP and UDP, instead of no answer (sing-box drops a query whose exchange
// errors and the client waits out its own timeout). SERVFAIL is not cached:
// the next query tries the upstreams again.
func TestTUNRemoteDNSAllFailAnswersServfail(t *testing.T) {
	socksPort, logged := startRemoteDNSBox(t, map[string]uint16{
		config.DNSRemoteTag:             freePort(t),
		config.DNSRemoteFallbackTags[0]: freePort(t),
		config.DNSRemoteFallbackTags[1]: freePort(t),
	})
	for _, c := range []struct {
		network string
		resolve func(*testing.T, uint16, string) (*dns.Msg, time.Duration)
	}{{"tcp", resolveTCP}, {"udp", resolveUDP}, {"udp again", resolveUDP}} {
		response, took := c.resolve(t, socksPort, "proxied.test.")
		if response.Rcode != dns.RcodeServerFailure || took > 11*time.Second {
			t.Fatalf("%s: rcode %s after %s", c.network, dns.RcodeToString[response.Rcode], took)
		}
	}
	lines := logged()
	if n := strings.Count(lines, "msg=dns name=proxied.test. type=A server=dns-remote-9.9.9.9 attempt=3 error="); n != 3 {
		t.Fatalf("%d complete fallbacks, want one per query (not cached):\n%s", n, lines)
	}
}

// startBlackhole accepts TCP connections and never answers, like a DoT
// server whose traffic is silently dropped.
func startBlackhole(t *testing.T) uint16 {
	t.Helper()
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	var mu sync.Mutex
	var conns []net.Conn
	go func() {
		for {
			conn, err := listener.Accept()
			if err != nil {
				return
			}
			mu.Lock()
			conns = append(conns, conn)
			mu.Unlock()
		}
	}()
	t.Cleanup(func() {
		listener.Close()
		mu.Lock()
		defer mu.Unlock()
		for _, conn := range conns {
			conn.Close()
		}
	})
	return uint16(listener.Addr().(*net.TCPAddr).Port)
}

// When every remote server times out (silently dropped), SERVFAIL still
// arrives within the guard budget: the budget ends before sing-box's own DNS
// timeout cancels the query.
func TestTUNRemoteDNSAllTimeOutAnswersServfail(t *testing.T) {
	if testing.Short() {
		t.Skip("waits out the guard budget")
	}
	if raceEnabled {
		// A DoT handshake that times out closes the Shadowsocks 2022 conn
		// while its first Write is sending the request header: an upstream
		// race (SagerNet/sing-shadowsocks2#9), not one in this code.
		t.Skip("upstream race in sing-shadowsocks2 (SagerNet/sing-shadowsocks2#9)")
	}
	socksPort, logged := startRemoteDNSBox(t, map[string]uint16{
		config.DNSRemoteTag:             startBlackhole(t),
		config.DNSRemoteFallbackTags[0]: startBlackhole(t),
		config.DNSRemoteFallbackTags[1]: startBlackhole(t),
	})
	response, took := resolveUDP(t, socksPort, "proxied.test.")
	if response.Rcode != dns.RcodeServerFailure || took < 7*time.Second || took > 9500*time.Millisecond {
		t.Fatalf("rcode %s after %s:\n%s", dns.RcodeToString[response.Rcode], took, logged())
	}
	if !strings.Contains(logged(), "server=dns-remote-9.9.9.9 attempt=3 error=") {
		t.Fatalf("not every upstream tried:\n%s", logged())
	}
}
