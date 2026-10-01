package config

import (
	"regexp"
	"slices"
	"testing"
	"time"

	"github.com/peakpassvpn/ppvpn-core/internal/dnstransport"
	"github.com/peakpassvpn/ppvpn-core/internal/domaindest"
	"github.com/peakpassvpn/ppvpn-core/profile"
	C "github.com/sagernet/sing-box/constant"
	"github.com/sagernet/sing-box/option"
)

func tunRoutingProfile(final string) *profile.Profile {
	p := base(node(profile.ProtocolShadowsocks))
	p.Routing = profile.Routing{
		Rules: []profile.RoutingRule{
			{ID: "cn", Match: profile.RoutingMatch{DomainSuffixes: []string{"cn.example"}}, Action: profile.RoutingAction{Type: "direct"}},
			{ID: "ads", Match: profile.RoutingMatch{Domains: []string{"ads.example"}}, Action: profile.RoutingAction{Type: "reject"}},
			{ID: "video", Match: profile.RoutingMatch{DomainSuffixes: []string{"video.example"}, Ports: []uint16{443}}, Action: profile.RoutingAction{Type: "proxy", Target: "node", NodeID: "stable"}},
			{ID: "lan", Match: profile.RoutingMatch{IPIsPrivate: true}, Action: profile.RoutingAction{Type: "direct"}},
		},
		Final: profile.RoutingAction{Type: final},
	}
	if final == "proxy" {
		p.Routing.Final.Target = "selected"
	}
	return p
}

func buildTUN(t *testing.T, platform string, final string) *BuildResult {
	t.Helper()
	got, err := Build(tunRoutingProfile(final), profile.PlatformCapabilities{Platform: platform, TUN: profile.TUNCapabilities{Enabled: true}}, time.Now())
	if err != nil {
		t.Fatal(err)
	}
	return got
}

// Desktop (auto_route) and mobile (platform-owned tunnel) TUN render the same
// sniff, DNS hijack and fake-ip rules ahead of everything else.
func TestTUNCoreRulesComeFirst(t *testing.T) {
	for _, platform := range []string{"windows", "linux", "macos", "ios", "android"} {
		t.Run(platform, func(t *testing.T) {
			rules := buildTUN(t, platform, "proxy").Options.Route.Rules
			if len(rules) < 6 {
				t.Fatalf("rules: %d", len(rules))
			}
			sniff := rules[0].DefaultOptions
			if rules[0].Type != C.RuleTypeDefault || sniff.Action != C.RuleActionTypeSniff || !slices.Equal(sniff.Inbound, []string{TUNInboundTag}) ||
				len(sniff.SniffOptions.Sniffer) != 0 || len(sniff.Domain)+len(sniff.IPCIDR)+len(sniff.Port) != 0 {
				t.Fatalf("sniff rule: %#v", rules[0])
			}
			byProtocol, byPort := rules[1].DefaultOptions, rules[2].DefaultOptions
			if byProtocol.Action != C.RuleActionTypeHijackDNS || !slices.Equal(byProtocol.Protocol, []string{C.ProtocolDNS}) || !slices.Equal(byProtocol.Inbound, []string{TUNInboundTag}) {
				t.Fatalf("hijack-dns by protocol: %#v", rules[1])
			}
			if byPort.Action != C.RuleActionTypeHijackDNS || !slices.Equal(byPort.Port, []uint16{53}) || !slices.Equal(byPort.Inbound, []string{TUNInboundTag}) {
				t.Fatalf("hijack-dns by port: %#v", rules[2])
			}
			// Non-DNS traffic to the tunnel's own networks is rejected (#17).
			self := rules[3].DefaultOptions
			if rules[3].Type != C.RuleTypeDefault || self.Action != C.RuleActionTypeReject || !slices.Equal(self.Inbound, []string{TUNInboundTag}) ||
				!slices.Equal(self.IPCIDR, []string{"10.60.159.88/30", "fde2:ec40:9312:c7fd::/126"}) {
				t.Fatalf("tunnel self reject: %#v", rules[3])
			}
			reject := rules[4].LogicalOptions
			if rules[4].Type != C.RuleTypeLogical || reject.Mode != C.LogicalTypeAnd || reject.Action != C.RuleActionTypeReject ||
				reject.RejectOptions.Method != C.RuleActionRejectMethodDefault || len(reject.Rules) != 2 {
				t.Fatalf("fake-ip reject: %#v", rules[4])
			}
			// The client baseline sends private and LAN destinations direct.
			baseline := rules[5].DefaultOptions
			if baseline.RouteOptions.Outbound != "direct" || !slices.Equal(baseline.Inbound, []string{TUNInboundTag}) || len(baseline.IPCIDR) != len(profile.PrivatePrefixes) ||
				!slices.Contains(baseline.IPCIDR, "224.0.0.0/4") || !slices.Contains(baseline.IPCIDR, "fe80::/10") {
				t.Fatalf("baseline: %#v", rules[5])
			}
			inRange, noDomain := reject.Rules[0].DefaultOptions, reject.Rules[1].DefaultOptions
			if !slices.Equal(inRange.IPCIDR, []string{"198.18.0.0/15"}) || !slices.Equal(inRange.Inbound, []string{TUNInboundTag}) || inRange.Invert {
				t.Fatalf("fake-ip range: %#v", inRange)
			}
			if !slices.Equal(noDomain.DomainRegex, []string{knownDomainRegex}) || !noDomain.Invert {
				t.Fatalf("no-domain condition: %#v", noDomain)
			}
		})
	}
}

func TestTUNDNSServersAndMirroredRules(t *testing.T) {
	got := buildTUN(t, "windows", "proxy")
	dns := got.Options.DNS
	if dns == nil || !dns.ReverseMapping || dns.Final != DNSRemoteTag || len(dns.Servers) != 4 {
		t.Fatalf("dns: %#v", dns)
	}
	local := dns.Servers[0]
	if local.Type != C.DNSTypeLocal || local.Tag != DNSLocalTag || local.Options.(*option.LocalDNSServerOptions).Detour != "" {
		t.Fatalf("local server: %#v", local)
	}
	// dns-remote, then its fallbacks: DoT to resolvers outside mainland
	// China, each through the selected node.
	for i, want := range []struct{ tag, server string }{
		{DNSRemoteTag, "1.1.1.1"}, {"dns-remote-8.8.8.8", "8.8.8.8"}, {"dns-remote-9.9.9.9", "9.9.9.9"},
	} {
		remote := dns.Servers[1+i]
		remoteOptions, ok := remote.Options.(*option.RemoteTLSDNSServerOptions)
		if remote.Type != C.DNSTypeTLS || remote.Tag != want.tag || !ok || remoteOptions.Server != want.server || remoteOptions.ServerPort != 0 || remoteOptions.Detour != selectedOutboundTag {
			t.Fatalf("remote server %d: %#v", i, remote)
		}
	}
	if resolver := got.Options.Route.DefaultDomainResolver; resolver == nil || resolver.Server != DNSLocalTag {
		t.Fatalf("default domain resolver: %#v", resolver)
	}
	type want struct {
		domains, suffixes []string
		action, server    string
	}
	wants := []want{
		{[]string{"edge.example.com"}, nil, C.RuleActionTypeRoute, DNSLocalTag}, // ingress safety rule
		{[]string{"cn.example"}, []string{".cn.example"}, C.RuleActionTypeRoute, DNSLocalTag},
		{[]string{"ads.example"}, nil, C.RuleActionTypeReject, ""},
		{[]string{"video.example"}, []string{".video.example"}, C.RuleActionTypeRoute, DNSRemoteTag},
	}
	if len(dns.Rules) != len(wants) {
		t.Fatalf("dns rules: %#v", dns.Rules)
	}
	for i, w := range wants {
		r := dns.Rules[i].DefaultOptions
		if !slices.Equal(r.Domain, w.domains) || !slices.Equal(r.DomainSuffix, w.suffixes) || r.Action != w.action || r.RouteOptions.Server != w.server {
			t.Fatalf("dns rule %d: %#v", i, r)
		}
		if r.Action == C.RuleActionTypeReject && r.RejectOptions.Method != C.RuleActionRejectMethodDefault {
			t.Fatalf("dns reject without method: %#v", r)
		}
	}
	if final := buildTUN(t, "linux", "direct").Options.DNS.Final; final != DNSLocalTag {
		t.Fatalf("direct final resolves through %q", final)
	}
	if final := buildTUN(t, "linux", "reject").Options.DNS.Final; final != DNSRemoteTag {
		t.Fatalf("reject final resolves through %q", final)
	}
}

func TestTUNProxyTargetsCarryDomain(t *testing.T) {
	got := buildTUN(t, "macos", "proxy")
	nodeTag := got.NodeTags["stable"]
	wrappers := map[string]string{}
	for _, o := range got.Options.Outbounds {
		if o.Type == domaindest.Type {
			options := o.Options.(*domaindest.Options)
			if !slices.Equal(options.Inbounds, []string{TUNInboundTag}) {
				t.Fatalf("wrapper inbounds: %#v", options)
			}
			wrappers[o.Tag] = options.Outbound
		}
	}
	if len(wrappers) != 2 || wrappers["domain-selected"] != selectedOutboundTag || wrappers["domain-"+nodeTag] != nodeTag {
		t.Fatalf("wrappers: %#v", wrappers)
	}
	if got.Options.Route.Final != "domain-selected" {
		t.Fatalf("final: %s", got.Options.Route.Final)
	}
	for _, r := range got.Options.Route.Rules {
		if slices.Contains(r.DefaultOptions.Domain, "video.example") && r.DefaultOptions.RouteOptions.Outbound != "domain-"+nodeTag {
			t.Fatalf("fixed node rule: %#v", r)
		}
		// Direct stays direct: only the proxy path hands the domain over.
		if slices.Contains(r.DefaultOptions.DomainSuffix, ".cn.example") && r.DefaultOptions.RouteOptions.Outbound != "direct" {
			t.Fatalf("direct rule: %#v", r)
		}
	}
}

// Without TUN (local proxy, system proxy, compatibility mode) none of it is
// rendered: those inbounds carry domains already.
func TestNoTUNKeepsRoutingUnchanged(t *testing.T) {
	got, err := Build(tunRoutingProfile("proxy"), profile.PlatformCapabilities{Platform: "windows"}, time.Now())
	if err != nil {
		t.Fatal(err)
	}
	if got.Options.DNS != nil || got.Options.Route.DefaultDomainResolver != nil || got.Options.Route.Final != selectedOutboundTag {
		t.Fatalf("route: %#v dns: %#v", got.Options.Route, got.Options.DNS)
	}
	for _, o := range got.Options.Outbounds {
		if o.Type == domaindest.Type {
			t.Fatalf("wrapper rendered without TUN: %s", o.Tag)
		}
	}
	for _, r := range got.Options.Route.Rules {
		if r.Type != C.RuleTypeDefault || r.DefaultOptions.Action == C.RuleActionTypeSniff || r.DefaultOptions.Action == C.RuleActionTypeHijackDNS {
			t.Fatalf("TUN rule rendered without TUN: %#v", r)
		}
		if r.DefaultOptions.Action == C.RuleActionTypeReject && r.DefaultOptions.RejectOptions.Method != C.RuleActionRejectMethodDefault {
			t.Fatalf("reject without method: %#v", r)
		}
	}
}

// The fake-ip rule must treat an IP literal (what the HTTP sniffer reports
// for a request to a bare address) as "no domain".
func TestKnownDomainRegexRejectsIPLiterals(t *testing.T) {
	re := regexp.MustCompile(knownDomainRegex)
	for _, domain := range []string{"example.com", "a.b", "fake.test", "1password.com", "123.example", "xn--fiqs8s.cn", "localhost", "a-1.2"} {
		if !re.MatchString(domain) {
			t.Errorf("%q should be a domain", domain)
		}
	}
	for _, literal := range []string{"", "198.18.1.117", "198.18.1.117.", "::1", "2001:db8::7", "[2001:db8::7]", "::ffff:198.18.1.1", "fe80::1%en0"} {
		if re.MatchString(literal) {
			t.Errorf("%q should not be a domain", literal)
		}
	}
}

// A host-supplied physical resolver becomes a UDP dns-local (the first one
// outside the tunnel); tunnel addresses are skipped; with none left dns-local
// stays sing-box's local transport; a malformed entry fails the build.
func TestLocalDNSServers(t *testing.T) {
	build := func(servers ...string) (*BuildResult, error) {
		return Build(base(node(profile.ProtocolShadowsocks)), profile.PlatformCapabilities{Platform: "macos", TUN: profile.TUNCapabilities{Enabled: true, LocalDNSServers: servers}}, time.Now())
	}
	cases := []struct {
		servers    []string
		wantServer string
		wantPort   uint16
	}{
		{[]string{"10.10.0.3"}, "10.10.0.3", 53},
		{[]string{"10.60.159.90", "fde2:ec40:9312:c7fd::2", "172.19.0.2", "fdfe:dcba:9876::2", "192.168.1.1:5353"}, "192.168.1.1", 5353},
		{[]string{"[fe80::1%en0]:53"}, "fe80::1%en0", 53},
		{[]string{"::ffff:10.0.0.1"}, "10.0.0.1", 53},
	}
	for _, tc := range cases {
		got, err := build(tc.servers...)
		if err != nil {
			t.Fatalf("%v: %v", tc.servers, err)
		}
		local := got.Options.DNS.Servers[0]
		options, ok := local.Options.(*option.RemoteDNSServerOptions)
		if local.Type != C.DNSTypeUDP || local.Tag != DNSLocalTag || !ok || options.Server != tc.wantServer || options.ServerPort != tc.wantPort || options.Detour != "" {
			t.Fatalf("%v: %#v", tc.servers, local)
		}
	}
	for _, servers := range [][]string{nil, {"10.60.159.90", "fde2:ec40:9312:c7fd::2", "172.19.0.2", "fdfe:dcba:9876::2"}} {
		got, err := build(servers...)
		if err != nil {
			t.Fatal(err)
		}
		if local := got.Options.DNS.Servers[0]; local.Type != C.DNSTypeLocal || local.Tag != DNSLocalTag {
			t.Fatalf("%v: %#v", servers, local)
		}
	}
	for _, bad := range []string{"dns.example", "10.0.0.1:0", "0.0.0.0", "[::1]:abc"} {
		if _, err := build(bad); err == nil {
			t.Fatalf("%q accepted", bad)
		}
	}
}

// The DNS transport guard is keyed on the remote server's tag.
func TestGuardedDNSTagIsTheRemoteServer(t *testing.T) {
	if dnstransport.GuardedTag != DNSRemoteTag {
		t.Fatalf("guarded %q, remote %q", dnstransport.GuardedTag, DNSRemoteTag)
	}
	if !slices.Equal(dnstransport.FallbackTags, DNSRemoteFallbackTags) {
		t.Fatalf("fallbacks %q, config %q", dnstransport.FallbackTags, DNSRemoteFallbackTags)
	}
}
