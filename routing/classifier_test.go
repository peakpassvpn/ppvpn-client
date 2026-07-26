package routing

import (
	"testing"
	"time"

	"github.com/peakpassvpn/ppvpn-core/profile"
)

func testProfile() *profile.Profile {
	node := profile.Node{
		ID:       "node-a",
		Protocol: profile.ProtocolShadowsocks,
		Endpoint: profile.Endpoint{Domain: "edge.example.com", IP: "8.8.8.8", Port: 443},
		Credentials: profile.Credentials{Shadowsocks: &profile.ShadowsocksCredentials{
			Method: "2022-blake3-aes-128-gcm", ServerKey: "AAAAAAAAAAAAAAAAAAAAAA==",
		}},
		Capabilities: profile.Capabilities{TCP: true, UDP: true},
	}
	other := node
	other.ID = "node-b"
	return &profile.Profile{
		SchemaVersion: profile.CurrentSchemaVersion,
		Revision:      "rules",
		ExpiresAt:     time.Now().Add(time.Hour),
		Nodes:         []profile.Node{node, other},
		Selection:     profile.Selection{Mode: "manual", DefaultNodeID: "node-a"},
		Routing: profile.Routing{
			Rules: []profile.RoutingRule{
				{ID: "unicode", Match: profile.RoutingMatch{DomainSuffixes: []string{"例子.测试"}}, Action: profile.RoutingAction{Type: "direct"}},
				{ID: "v6", Match: profile.RoutingMatch{IPCIDRs: []string{"2001:4860::/32"}, Protocols: []string{"udp"}, Ports: []uint16{53}}, Action: profile.RoutingAction{Type: "reject"}},
				{ID: "private", Match: profile.RoutingMatch{IPIsPrivate: true}, Action: profile.RoutingAction{Type: "direct"}},
				{ID: "range", Match: profile.RoutingMatch{PortRanges: []string{"8000-9000"}}, Action: profile.RoutingAction{Type: "proxy", Target: "node", NodeID: "node-b"}},
			},
			Final: profile.RoutingAction{Type: "proxy", Target: "selected"},
		},
	}
}

func TestFirstMatchDomainIDNAAndLabelBoundary(t *testing.T) {
	classifier, err := Compile(testProfile(), time.Now())
	if err != nil {
		t.Fatal(err)
	}
	for _, hostname := range []string{"例子.测试.", "A.例子.测试", "xn--fsqu00a.xn--0zwm56d"} {
		got := classifier.Classify(Flow{Hostname: hostname, Protocol: "tcp", DestinationPort: 443}, "node-a")
		if got.Type != "direct" || got.RuleID != "unicode" {
			t.Fatalf("%q: %#v", hostname, got)
		}
	}
	got := classifier.Classify(Flow{Hostname: "bad例子.测试", Protocol: "tcp", DestinationPort: 443}, "node-a")
	if got.Type != "proxy" || got.Priority != "final" {
		t.Fatalf("suffix crossed label boundary: %#v", got)
	}
	got = classifier.Classify(Flow{Hostname: "192.0.2.1", Protocol: "tcp", DestinationPort: 443}, "node-a")
	if got.RuleID == "unicode" {
		t.Fatalf("IP literal entered domain matching: %#v", got)
	}
}

func TestCIDRProtocolPortPrivateAndRange(t *testing.T) {
	classifier, err := Compile(testProfile(), time.Now())
	if err != nil {
		t.Fatal(err)
	}
	got := classifier.Classify(Flow{DestinationIP: "2001:4860::1", Protocol: "udp", DestinationPort: 53}, "node-a")
	if got.Type != "reject" || got.RuleID != "v6" {
		t.Fatalf("v6: %#v", got)
	}
	got = classifier.Classify(Flow{DestinationIP: "10.0.0.1", Protocol: "tcp", DestinationPort: 443}, "node-a")
	if got.Type != "direct" || got.RuleID != "private" {
		t.Fatalf("private: %#v", got)
	}
	got = classifier.Classify(Flow{DestinationIP: "1.1.1.1", Protocol: "tcp", DestinationPort: 8443}, "node-a")
	if got.NodeID != "node-b" || got.RuleID != "range" {
		t.Fatalf("range: %#v", got)
	}
}

func TestFixedPriorityAndSelectedSnapshot(t *testing.T) {
	classifier, err := Compile(testProfile(), time.Now())
	if err != nil {
		t.Fatal(err)
	}
	flow := Flow{Hostname: "例子.测试", Protocol: "tcp", DestinationPort: 443}
	safety := flow
	safety.Entry = EntryPlatformSafety
	if got := classifier.Classify(safety, "node-a"); got.Type != "direct" || got.Priority != "platform_safety" {
		t.Fatalf("safety: %#v", got)
	}
	local := flow
	local.Entry = EntryLocalProxy
	local.LocalProxyNodeID = "node-b"
	if got := classifier.Classify(local, "node-a"); got.NodeID != "node-b" || got.Priority != "local_proxy" {
		t.Fatalf("local: %#v", got)
	}
	if got := classifier.Classify(Flow{Hostname: "other.example", Protocol: "tcp", DestinationPort: 443}, "node-b"); got.NodeID != "node-b" || got.Priority != "final" {
		t.Fatalf("selected: %#v", got)
	}
}

func TestAddressMatchersAreORWhileProtocolAndPortAreAND(t *testing.T) {
	p := testProfile()
	p.Routing.Rules = []profile.RoutingRule{{
		ID: "address-category",
		Match: profile.RoutingMatch{
			DomainSuffixes: []string{"example.com"},
			IPCIDRs:        []string{"1.1.1.0/24"},
			Protocols:      []string{"tcp"},
			Ports:          []uint16{443},
		},
		Action: profile.RoutingAction{Type: "direct"},
	}}
	classifier, err := Compile(p, time.Now())
	if err != nil {
		t.Fatal(err)
	}
	for _, flow := range []Flow{
		{Hostname: "a.example.com", DestinationIP: "8.8.8.8", Protocol: "tcp", DestinationPort: 443},
		{Hostname: "other.example", DestinationIP: "1.1.1.1", Protocol: "tcp", DestinationPort: 443},
	} {
		if got := classifier.Classify(flow, "node-a"); got.Type != "direct" {
			t.Fatalf("address alternative did not match: %#v", got)
		}
	}
	if got := classifier.Classify(Flow{Hostname: "a.example.com", Protocol: "udp", DestinationPort: 443}, "node-a"); got.Type != "proxy" {
		t.Fatalf("protocol was not ANDed: %#v", got)
	}
	if got := classifier.Classify(Flow{Hostname: "a.example.com", Protocol: "tcp", DestinationPort: 80}, "node-a"); got.Type != "proxy" {
		t.Fatalf("port was not ANDed: %#v", got)
	}
}
