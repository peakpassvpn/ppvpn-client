package runtime

import (
	"context"
	"errors"
	"fmt"
	"github.com/sagernet/sing/common/control"
	"net"
	"net/netip"
	"strings"

	"github.com/peakpassvpn/ppvpn-core/internal/config"
	"github.com/peakpassvpn/ppvpn-core/internal/corelog"
	"github.com/peakpassvpn/ppvpn-core/internal/failover"
	"github.com/peakpassvpn/ppvpn-core/profile"
	"github.com/peakpassvpn/ppvpn-core/routing"
	"github.com/sagernet/sing-box/option"
	"sync"
	"testing"
	"time"
)

type fakeEngine struct {
	startErr        error
	started, closed bool
	selected        string
	dialConn        net.Conn
	dialNetwork     string
	dialOutbound    string
	dialHost        string
	dialPort        uint16
}

func TestConcurrentLifecycleOperationsDoNotLeakEngines(t *testing.T) {
	factory := &fakeFactory{}
	core := newCore(profile.PlatformCapabilities{}, factory.create)
	if _, err := core.ApplyProfile(testProfile("initial", "a.example", "8.8.8.8"), time.Now()); err != nil {
		t.Fatal(err)
	}
	var wg sync.WaitGroup
	for worker := 0; worker < 4; worker++ {
		worker := worker
		wg.Add(1)
		go func() {
			defer wg.Done()
			for i := 0; i < 20; i++ {
				switch (worker + i) % 4 {
				case 0:
					_ = core.Start()
				case 1:
					_ = core.Stop()
				case 2:
					p := testProfile(fmt.Sprintf("w%d-%d", worker, i), "a.example", "8.8.8.8")
					_, _ = core.ApplyProfile(p, time.Now())
				case 3:
					_ = core.Reload()
				}
			}
		}()
	}
	wg.Wait()
	if err := core.Stop(); err != nil {
		t.Fatal(err)
	}
	for i, engine := range factory.engines {
		if engine.started && !engine.closed {
			t.Fatalf("engine %d was left running", i)
		}
	}
}

func (e *fakeEngine) Start() error { e.started = true; return e.startErr }
func (e *fakeEngine) Close() error { e.closed = true; return nil }
func (e *fakeEngine) dialFlow(_ context.Context, network, outbound, host string, port uint16) (net.Conn, error) {
	e.dialNetwork, e.dialOutbound, e.dialHost, e.dialPort = network, outbound, host, port
	if e.dialConn == nil {
		return nil, errors.New("not implemented")
	}
	return e.dialConn, nil
}
func (e *fakeEngine) activeIngress(nodeTag string) (failover.Active, bool) {
	return failover.Active{Current: nodeTag}, true
}
func (e *fakeEngine) selectOutbound(tag string) bool {
	e.selected = tag
	return true
}

type fakeFactory struct {
	engines []*fakeEngine
	nextErr error
}

func (f *fakeFactory) create(_ context.Context, _ option.Options) (engine, error) {
	e := &fakeEngine{startErr: f.nextErr}
	f.nextErr = nil
	f.engines = append(f.engines, e)
	return e, nil
}

// The desktop TUN carries IPv6 only when the host probe allows it; the probe
// runs on every apply because IPv6 can be toggled between starts.
func TestApplyProfileProbesHostIPv6ForTUN(t *testing.T) {
	core := newCore(profile.PlatformCapabilities{Platform: "linux", TUN: profile.TUNCapabilities{Enabled: true}}, (&fakeFactory{}).create)
	for _, ipv6 := range []bool{false, true} {
		core.hostIPv6 = func() bool { return ipv6 }
		if _, err := core.ApplyProfile(testProfile(fmt.Sprintf("ipv6-%v", ipv6), "a.example", "8.8.8.8"), time.Now()); err != nil {
			t.Fatal(err)
		}
		core.mu.RLock()
		inbounds := core.built.Options.Inbounds
		core.mu.RUnlock()
		tun := inbounds[len(inbounds)-1].Options.(*option.TunInboundOptions)
		if want := map[bool]int{false: 1, true: 2}[ipv6]; len(tun.Address) != want {
			t.Fatalf("host ipv6=%v: address %v", ipv6, tun.Address)
		}
	}
}

// A host with IPv6 enabled but no IPv6 path keeps the IPv6 TUN and wraps
// direct; the probe runs on every apply and again at start, and its result
// is logged. A failed probe keeps IPv6 as before and says why.
func TestHostIPv6RouteDecidesDirectHandOff(t *testing.T) {
	var b strings.Builder
	core := newCore(profile.PlatformCapabilities{Platform: "windows", TUN: profile.TUNCapabilities{Enabled: true}}, (&fakeFactory{}).create)
	core.SetLogger(corelog.New(&b))
	core.hostIPv6 = func() bool { return true }
	route, routeErr := false, error(nil)
	core.hostIPv6Route = func() (bool, error) { return route, routeErr }
	built := func() *config.BuildResult {
		core.mu.RLock()
		defer core.mu.RUnlock()
		return core.built
	}

	if _, err := core.ApplyProfile(testProfile("no-route", "a.example", "8.8.8.8"), time.Now()); err != nil {
		t.Fatal(err)
	}
	if !built().DirectIPv6HandOff {
		t.Fatal("no IPv6 route: direct not wrapped")
	}
	tun := built().Options.Inbounds[len(built().Options.Inbounds)-1].Options.(*option.TunInboundOptions)
	if len(tun.Address) != 2 {
		t.Fatalf("no IPv6 route: TUN lost its IPv6 address: %v", tun.Address)
	}
	if !strings.Contains(b.String(), `msg="host ipv6" host_ipv6_enabled=true host_ipv6_route=false policy=tun_ipv6_direct_ipv4`) {
		t.Fatalf("log:\n%s", b.String())
	}

	// The host joins an IPv6 network while stopped: start rebuilds.
	route = true
	if err := core.Start(); err != nil {
		t.Fatal(err)
	}
	if built().DirectIPv6HandOff {
		t.Fatal("start kept the hand-off after the host gained an IPv6 route")
	}
	if err := core.Stop(); err != nil {
		t.Fatal(err)
	}

	// A failed probe: IPv6 as before, with a warning.
	route, routeErr = true, errors.New("route table unreadable")
	if _, err := core.ApplyProfile(testProfile("probe-failed", "a.example", "8.8.8.8"), time.Now()); err != nil {
		t.Fatal(err)
	}
	if built().DirectIPv6HandOff {
		t.Fatal("failed probe: direct wrapped")
	}
	if !strings.Contains(b.String(), `msg="host ipv6 route probe failed" error="route table unreadable" assumed_route=true`) {
		t.Fatalf("log:\n%s", b.String())
	}
}

// Apply and start each write one info line with per-phase durations, so a
// slow /v1/start shows where the time went.
func TestLifecycleLogsPhaseTimings(t *testing.T) {
	var b strings.Builder
	core := newCore(profile.PlatformCapabilities{Platform: "linux", TUN: profile.TUNCapabilities{Enabled: true}}, (&fakeFactory{}).create)
	core.hostIPv6 = func() bool { return true }
	core.SetLogger(corelog.New(&b))
	if _, err := core.ApplyProfile(testProfile("timing", "a.example", "8.8.8.8"), time.Now()); err != nil {
		t.Fatal(err)
	}
	if err := core.Start(); err != nil {
		t.Fatal(err)
	}
	defer core.Stop()
	lines := b.String()
	for _, want := range []string{
		"msg=\"apply timing\" outcome=ok tun=true rebuild=false rule_sets_ready=0 rule_sets_stale=0 rule_sets_unavailable=0 validate_ms=", "rule_sets_ms=", "host_ipv6_ms=", "build_ms=", "routing_ms=",
		"msg=\"start timing\" outcome=ok tun=true engine_create_ms=", "engine_start_ms=", "total_ms=",
	} {
		if !strings.Contains(lines, want) {
			t.Fatalf("missing %q in:\n%s", want, lines)
		}
	}
}

func testProfile(rev, domain, ip string) *profile.Profile {
	n := profile.Node{ID: "node", EntryKey: "cn-optimized", Capabilities: profile.Capabilities{TCP: true}, Ingresses: []profile.Ingress{{Role: profile.IngressRolePrimary, EndpointKey: "9001", Protocol: profile.ProtocolShadowsocks, Endpoint: profile.Endpoint{Domain: domain, IP: ip, Port: 443}, Credentials: profile.Credentials{Shadowsocks: &profile.ShadowsocksCredentials{Method: "2022-blake3-aes-128-gcm", UserKey: "AAAAAAAAAAAAAAAAAAAAAA=="}}, Capabilities: profile.Capabilities{TCP: true}}}}
	return &profile.Profile{SchemaVersion: profile.CurrentSchemaVersion, Revision: rev, ExpiresAt: time.Now().Add(time.Hour), Nodes: []profile.Node{n}, Selection: profile.Selection{Mode: "manual", DefaultNodeID: "node"}, Routing: profile.Routing{Final: profile.RoutingAction{Type: "proxy", Target: "selected"}}}
}
func TestAtomicApplyRollback(t *testing.T) {
	c := New(profile.PlatformCapabilities{})
	if ok, err := c.ApplyProfile(testProfile("r1", "a.example", "8.8.8.8"), time.Now()); err != nil || !ok {
		t.Fatal(err)
	}
	bad := testProfile("r2", "b.example", "198.18.0.1")
	if ok, err := c.ApplyProfile(bad, time.Now()); err == nil || ok {
		t.Fatal("invalid candidate applied")
	}
	if got := c.Status().Revision; got != "r1" {
		t.Fatalf("rolled forward to %s", got)
	}
}
func TestSameRevisionNoopAndMigrationKeepsSelection(t *testing.T) {
	c := New(profile.PlatformCapabilities{})
	p := testProfile("r1", "a.example", "8.8.8.8")
	c.ApplyProfile(p, time.Now())
	if ok, err := c.ApplyProfile(p, time.Now()); err != nil || ok {
		t.Fatal("same revision applied")
	}
	moved := testProfile("r2", "b.example", "1.1.1.1")
	if ok, err := c.ApplyProfile(moved, time.Now()); err != nil || !ok {
		t.Fatal(err)
	}
	if c.Status().SelectedNodeID != "node" {
		t.Fatal("selection lost")
	}
}

func TestLifecycleAndRuntimeRollback(t *testing.T) {
	factory := &fakeFactory{}
	c := newCore(profile.PlatformCapabilities{}, factory.create)
	if _, err := c.ApplyProfile(testProfile("r1", "a.example", "8.8.8.8"), time.Now()); err != nil {
		t.Fatal(err)
	}
	if err := c.Start(); err != nil || c.Status().State != StateRunning || !factory.engines[0].started {
		t.Fatalf("start: %v %#v", err, c.Status())
	}
	factory.nextErr = errors.New("boom")
	if ok, err := c.ApplyProfile(testProfile("r2", "b.example", "1.1.1.1"), time.Now()); err == nil || ok {
		t.Fatal("failed replacement applied")
	}
	if c.Status().Revision != "r1" || factory.engines[0].closed {
		t.Fatal("old runtime was not preserved")
	}
	if err := c.Stop(); err != nil || !factory.engines[0].closed || c.Status().State != StateConfigured {
		t.Fatalf("stop: %v %#v", err, c.Status())
	}
}

func TestProfileIsCopiedBeforeRetention(t *testing.T) {
	c := newCore(profile.PlatformCapabilities{}, (&fakeFactory{}).create)
	p := testProfile("r1", "a.example", "8.8.8.8")
	if _, err := c.ApplyProfile(p, time.Now()); err != nil {
		t.Fatal(err)
	}
	p.Nodes[0].ID = "mutated"
	if got := c.Nodes()[0].ID; got != "node" {
		t.Fatalf("retained caller memory: %s", got)
	}
}

func TestSelectNodeChangesOnlyNewFlowSelection(t *testing.T) {
	factory := &fakeFactory{}
	c := newCore(profile.PlatformCapabilities{}, factory.create)
	p := testProfile("r1", "a.example", "8.8.8.8")
	second := cloneNode(p.Nodes[0])
	second.ID = "second"
	second.Ingresses[0].EndpointKey = "9002"
	second.Ingresses[0].Endpoint.Domain = "b.example"
	second.Ingresses[0].Endpoint.IP = "1.1.1.1"
	p.Nodes = append(p.Nodes, second)
	if _, err := c.ApplyProfile(p, time.Now()); err != nil {
		t.Fatal(err)
	}
	if err := c.Start(); err != nil {
		t.Fatal(err)
	}
	engine := factory.engines[0]
	if err := c.SelectNode("second"); err != nil {
		t.Fatal(err)
	}
	if len(factory.engines) != 1 || engine.closed {
		t.Fatal("node selection restarted or closed the runtime")
	}
	if engine.selected != c.built.NodeTags["second"] || c.Status().SelectedNodeID != "second" {
		t.Fatalf("selection was not applied: %q %#v", engine.selected, c.Status())
	}
}

func TestTransparentFlowAdapterUsesCompiledDecisionAndNodeOutbound(t *testing.T) {
	factory := &fakeFactory{}
	c := newCore(profile.PlatformCapabilities{}, factory.create)
	p := testProfile("r1", "a.example", "8.8.8.8")
	p.Routing.Rules = []profile.RoutingRule{{
		ID:     "direct-private",
		Match:  profile.RoutingMatch{IPIsPrivate: true},
		Action: profile.RoutingAction{Type: "direct"},
	}}
	if _, err := c.ApplyProfile(p, time.Now()); err != nil {
		t.Fatal(err)
	}
	if err := c.Start(); err != nil {
		t.Fatal(err)
	}
	direct, err := c.ClassifyFlow(routing.Flow{
		Entry:           routing.EntryTransparent,
		DestinationIP:   "10.0.0.1",
		DestinationPort: 443,
		Protocol:        "tcp",
	})
	if err != nil || direct.Type != "direct" || direct.RuleID != "direct-private" {
		t.Fatalf("direct decision: %#v %v", direct, err)
	}
	local, peer := net.Pipe()
	defer peer.Close()
	factory.engines[0].dialConn = local
	flow := routing.Flow{
		Entry:           routing.EntryTransparent,
		Hostname:        "例子.测试.",
		DestinationPort: 443,
		Protocol:        "tcp",
	}
	decision, err := c.ClassifyFlow(flow)
	if err != nil {
		t.Fatal(err)
	}
	connection, err := c.OpenFlow(context.Background(), flow, decision)
	if err != nil {
		t.Fatal(err)
	}
	defer connection.Close()
	engine := factory.engines[0]
	if decision.NodeID != "node" ||
		engine.dialNetwork != "tcp" ||
		engine.dialOutbound != c.built.NodeTags["node"] ||
		engine.dialHost != "xn--fsqu00a.xn--0zwm56d" ||
		engine.dialPort != 443 {
		t.Fatalf("decision=%#v engine=%#v", decision, engine)
	}
}

func TestOpenFlowHonorsAuthorizedClassificationAcrossSelectedSwitch(t *testing.T) {
	factory := &fakeFactory{}
	c := newCore(profile.PlatformCapabilities{}, factory.create)
	p := testProfile("r1", "a.example", "8.8.8.8")
	second := cloneNode(p.Nodes[0])
	second.ID = "second"
	second.Ingresses[0].EndpointKey = "9002"
	second.Ingresses[0].Endpoint.Domain = "b.example"
	second.Ingresses[0].Endpoint.IP = "1.1.1.1"
	p.Nodes = append(p.Nodes, second)
	if _, err := c.ApplyProfile(p, time.Now()); err != nil {
		t.Fatal(err)
	}
	if err := c.Start(); err != nil {
		t.Fatal(err)
	}
	flow := routing.Flow{
		Entry:           routing.EntryTransparent,
		Hostname:        "destination.example",
		DestinationPort: 443,
		Protocol:        "tcp",
	}
	decision, err := c.ClassifyFlow(flow)
	if err != nil || decision.NodeID != "node" {
		t.Fatalf("classification: %#v %v", decision, err)
	}
	if err = c.SelectNode("second"); err != nil {
		t.Fatal(err)
	}
	local, peer := net.Pipe()
	defer peer.Close()
	factory.engines[0].dialConn = local
	connection, err := c.OpenFlow(context.Background(), flow, decision)
	if err != nil {
		t.Fatal(err)
	}
	defer connection.Close()
	if factory.engines[0].dialOutbound != c.built.NodeTags["node"] {
		t.Fatalf("open changed the authorized node: %q", factory.engines[0].dialOutbound)
	}

	tampered := decision
	tampered.NodeID = "second"
	if _, err = c.OpenFlow(context.Background(), flow, tampered); err == nil {
		t.Fatal("tampered decision was accepted")
	}
}

func TestOpenFlowRejectsDecisionFromOldProfileSnapshot(t *testing.T) {
	factory := &fakeFactory{}
	c := newCore(profile.PlatformCapabilities{}, factory.create)
	if _, err := c.ApplyProfile(testProfile("r1", "a.example", "8.8.8.8"), time.Now()); err != nil {
		t.Fatal(err)
	}
	if err := c.Start(); err != nil {
		t.Fatal(err)
	}
	flow := routing.Flow{Entry: routing.EntryTransparent, Hostname: "destination.example", DestinationPort: 443, Protocol: "tcp"}
	decision, err := c.ClassifyFlow(flow)
	if err != nil {
		t.Fatal(err)
	}
	if _, err = c.ApplyProfile(testProfile("r2", "b.example", "1.1.1.1"), time.Now()); err != nil {
		t.Fatal(err)
	}
	if _, err = c.OpenFlow(context.Background(), flow, decision); err == nil {
		t.Fatal("stale profile decision was accepted")
	}
}

func cloneNode(n profile.Node) profile.Node {
	n.Ingresses = append([]profile.Ingress(nil), n.Ingresses...)
	return n
}

// At debug level an apply logs fingerprints of each REALITY ingress's
// parameters, never the values themselves.
func TestApplyLogsRealityFingerprintsAtDebug(t *testing.T) {
	p := testProfile("reality", "edge.example.com", "8.8.8.8")
	key, shortID := "AQIDBAUGBwgJCgsMDQ4PEBESExQVFhcYGRobHB0eHyA", "0123456789abcdef"
	p.Nodes[0].Ingresses[0].Protocol = profile.ProtocolVLESS
	p.Nodes[0].Ingresses[0].Credentials = profile.Credentials{VLESS: &profile.VLESSCredentials{UUID: "00000000-0000-4000-8000-000000000001", Flow: "xtls-rprx-vision"}}
	p.Nodes[0].Ingresses[0].TLS = &profile.TLS{ServerName: "www.microsoft.com", Reality: &profile.Reality{PublicKey: key, ShortID: shortID}}
	for _, debug := range []bool{false, true} {
		var b strings.Builder
		log := corelog.New(&b)
		if debug {
			_ = log.SetLevel(corelog.LevelDebug)
		}
		core := newCore(profile.PlatformCapabilities{}, (&fakeFactory{}).create)
		core.SetLogger(log)
		applied, err := core.ApplyProfile(p, time.Now())
		if err != nil || !applied {
			t.Fatalf("apply: %v %v", applied, err)
		}
		logged := b.String()
		if !debug {
			if strings.Contains(logged, "ingress tls") {
				t.Fatalf("logged at info: %s", logged)
			}
			continue
		}
		want := `msg="ingress tls" node_id=node endpoint_key=9001 protocol=vless server_name=www.microsoft.com public_key_sha256=` + shortDigest(key) +
			` public_key_len=43 public_key_encoding="unpadded url-or-std" short_id_sha256=` + shortDigest(shortID) + ` short_id_len=16 fingerprint=chrome flow=xtls-rprx-vision`
		if !strings.Contains(logged, want) {
			t.Fatalf("want %q in:\n%s", want, logged)
		}
		if strings.Contains(logged, key) || strings.Contains(logged, shortID) {
			t.Fatalf("raw REALITY values logged:\n%s", logged)
		}
	}
	for value, want := range map[string]string{"ab+c=": "padded std", "a+b": "unpadded std", "a-b": "unpadded url", "abc": "unpadded url-or-std"} {
		if got := base64Flavor(value); got != want {
			t.Errorf("%q: %q", value, got)
		}
	}
}

func TestDefaultInterfaceLogLine(t *testing.T) {
	var b strings.Builder
	log := corelog.New(&b)
	logDefaultInterface(log, "start", &control.Interface{Name: "en0", Index: 6, MTU: 1500, Addresses: []netip.Prefix{netip.MustParsePrefix("192.168.50.22/24"), netip.MustParsePrefix("fe80::1/64")}})
	logDefaultInterface(log, "changed", nil)
	logged := b.String()
	for _, want := range []string{
		`msg="default interface" event=start name=en0 index=6 mtu=1500 addresses=192.168.50.22/24,fe80::1/64`,
		`msg="default interface" event=changed name=none`,
	} {
		if !strings.Contains(logged, want) {
			t.Fatalf("missing %q in:\n%s", want, logged)
		}
	}
}
