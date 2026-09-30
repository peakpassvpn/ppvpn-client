package routing

import (
	"strings"
	"testing"
	"time"

	"github.com/peakpassvpn/ppvpn-core/profile"
)

func testProfile() *profile.Profile {
	node := profile.Node{
		ID:           "node-a",
		EntryKey:     "cn-optimized",
		Capabilities: profile.Capabilities{TCP: true, UDP: true},
		Ingresses: []profile.Ingress{{
			Role:        profile.IngressRolePrimary,
			EndpointKey: "9001",
			Protocol:    profile.ProtocolShadowsocks,
			Endpoint:    profile.Endpoint{Domain: "edge.example.com", IP: "8.8.8.8", Port: 443},
			Credentials: profile.Credentials{Shadowsocks: &profile.ShadowsocksCredentials{
				Method: "2022-blake3-aes-128-gcm", UserKey: "AAAAAAAAAAAAAAAAAAAAAA==",
			}},
			Capabilities: profile.Capabilities{TCP: true, UDP: true},
		}},
	}
	other := node
	other.ID = "node-b"
	other.Ingresses = append([]profile.Ingress(nil), node.Ingresses...)
	other.Ingresses[0].EndpointKey = "9002"
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

func TestClassifierSkipsRuleSetOnlyRules(t *testing.T) {
	p := testProfile()
	p.Routing.RuleSets = []profile.RuleSet{{ID: "cn", URL: "https://api.example.com/cn.srs", SHA256: strings.Repeat("a", 64)}}
	p.Routing.Rules = []profile.RoutingRule{
		{ID: "set-only", Match: profile.RoutingMatch{RuleSetIDs: []string{"cn"}}, Action: profile.RoutingAction{Type: "reject"}},
		{ID: "mixed", Match: profile.RoutingMatch{RuleSetIDs: []string{"cn"}, Domains: []string{"example.com"}}, Action: profile.RoutingAction{Type: "direct"}},
	}
	c, err := Compile(p, time.Now())
	if err != nil {
		t.Fatal(err)
	}
	if d := c.Classify(Flow{Entry: EntryTransparent, Hostname: "other.test", DestinationPort: 443, Protocol: "tcp"}, "node-a"); d.RuleID != "" {
		t.Fatalf("rule-set-only rule matched: %+v", d)
	}
	if d := c.Classify(Flow{Entry: EntryTransparent, Hostname: "example.com", DestinationPort: 443, Protocol: "tcp"}, "node-a"); d.RuleID != "mixed" {
		t.Fatalf("mixed rule lost its domain matcher: %+v", d)
	}
}

// ip_is_private matches the same ranges as the sing-box rules
// (profile.PrivatePrefixes), not just Go's RFC 1918/ULA IsPrivate.
func TestClassifierPrivateMatchesSharedRanges(t *testing.T) {
	classifier, err := Compile(testProfile(), time.Now())
	if err != nil {
		t.Fatal(err)
	}
	for _, ip := range []string{"100.64.1.1", "169.254.1.1", "224.0.0.251", "239.255.255.250", "255.255.255.255", "127.0.0.1", "fe80::1", "ff02::fb", "::1", "fd00::1", "::ffff:192.168.1.1"} {
		if got := classifier.Classify(Flow{DestinationIP: ip, Protocol: "udp", DestinationPort: 5353}, "node-a"); got.RuleID != "private" {
			t.Errorf("%s: %#v", ip, got)
		}
	}
	for _, ip := range []string{"1.1.1.1", "2606:4700::1111", "198.51.100.1"} {
		if got := classifier.Classify(Flow{DestinationIP: ip, Protocol: "udp", DestinationPort: 5353}, "node-a"); got.RuleID == "private" {
			t.Errorf("%s matched private: %#v", ip, got)
		}
	}
}
