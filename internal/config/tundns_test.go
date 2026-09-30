package config

import (
	"regexp"
	"slices"
	"testing"
	"time"

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
func TestTUNSniffHijackAndFakeIPRejectComeFirst(t *testing.T) {
	for _, platform := range []string{"windows", "linux", "macos", "ios", "android"} {
		t.Run(platform, func(t *testing.T) {
			rules := buildTUN(t, platform, "proxy").Options.Route.Rules
			if len(rules) < 4 {
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
			reject := rules[3].LogicalOptions
			if rules[3].Type != C.RuleTypeLogical || reject.Mode != C.LogicalTypeAnd || reject.Action != C.RuleActionTypeReject ||
				reject.RejectOptions.Method != C.RuleActionRejectMethodDefault || len(reject.Rules) != 2 {
				t.Fatalf("fake-ip reject: %#v", rules[3])
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
	if dns == nil || !dns.ReverseMapping || dns.Final != DNSRemoteTag || len(dns.Servers) != 2 {
		t.Fatalf("dns: %#v", dns)
	}
	local, remote := dns.Servers[0], dns.Servers[1]
	if local.Type != C.DNSTypeLocal || local.Tag != DNSLocalTag || local.Options.(*option.LocalDNSServerOptions).Detour != "" {
		t.Fatalf("local server: %#v", local)
	}
	remoteOptions, ok := remote.Options.(*option.RemoteHTTPSDNSServerOptions)
	if remote.Type != C.DNSTypeHTTPS || remote.Tag != DNSRemoteTag || !ok || remoteOptions.Server != "1.1.1.1" || remoteOptions.Detour != selectedOutboundTag {
		t.Fatalf("remote server: %#v", remote)
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
