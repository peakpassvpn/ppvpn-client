package config

import (
	"github.com/peakpassvpn/ppvpn-core/internal/domaindest"
	"github.com/peakpassvpn/ppvpn-core/profile"
	C "github.com/sagernet/sing-box/constant"
	"github.com/sagernet/sing-box/option"
	"github.com/sagernet/sing/common/json/badoption"
)

// Enhanced (TUN) mode sees raw IP packets, so it has to supply the two things
// the local proxies get for free from HTTP CONNECT/SOCKS: the domain of each
// connection, and a DNS answer the OS resolver can trust. Everything below is
// rendered only when TUN is enabled; local proxy, system proxy and
// compatibility mode keep their behavior.
//
//   - route rule 0 sniffs every TUN connection (all sniffers: TLS SNI, HTTP
//     Host, QUIC, DNS, ...), so profile domain rules match.
//   - route rules 1-2 hijack DNS (sniffed protocol dns, or port 53) into the
//     DNS module. The TUN announces its peer address (172.19.0.2) as the
//     interface DNS server, so every OS query is answered by the core and none
//     leaks around the tunnel.
//   - route rule 3 rejects a TUN connection to 198.18.0.0/15 whose domain is
//     unknown: that is a LAN fake-ip answer no node can reach, and proxying it
//     would hang for the node's whole connect timeout.
//   - DNS rules mirror the route rules: a domain routed direct resolves
//     through the system resolver dialed direct ("dns-local"); a domain routed
//     to a proxy resolves through DoT to 1.1.1.1 dialed through the selected
//     node ("dns-remote"); a rejected domain is refused. dns.final follows
//     route.final. reverse_mapping remembers which domain each answered
//     address belongs to.
//   - every proxy route target is wrapped by domaindest, which hands the known
//     domain (sniffed, or from DNS) to the node instead of the address.

// TUNInboundTag is the tag of the TUN inbound.
const TUNInboundTag = "tun"

const (
	DNSLocalTag  = "dns-local"
	DNSRemoteTag = "dns-remote"
	// RemoteDNSServer is queried over DoT through the selected node.
	RemoteDNSServer = "1.1.1.1"
	// FakeIPRange is the benchmark range fake-ip DNS servers answer from.
	FakeIPRange = "198.18.0.0/15"
	// domainDestinationPrefix names the wrapper of a proxy route target.
	domainDestinationPrefix = "domain-"
)

// addTUNTrafficRules renders the TUN-only rules that precede every other
// route rule.
func addTUNTrafficRules(result *BuildResult) {
	tun := badoption.Listable[string]{TUNInboundTag}
	result.Options.Route.Rules = append(result.Options.Route.Rules,
		option.Rule{Type: C.RuleTypeDefault, DefaultOptions: option.DefaultRule{
			RawDefaultRule: option.RawDefaultRule{Inbound: tun},
			RuleAction:     option.RuleAction{Action: C.RuleActionTypeSniff},
		}},
		option.Rule{Type: C.RuleTypeDefault, DefaultOptions: option.DefaultRule{
			RawDefaultRule: option.RawDefaultRule{Inbound: tun, Protocol: badoption.Listable[string]{C.ProtocolDNS}},
			RuleAction:     option.RuleAction{Action: C.RuleActionTypeHijackDNS},
		}},
		option.Rule{Type: C.RuleTypeDefault, DefaultOptions: option.DefaultRule{
			RawDefaultRule: option.RawDefaultRule{Inbound: tun, Port: badoption.Listable[uint16]{53}},
			RuleAction:     option.RuleAction{Action: C.RuleActionTypeHijackDNS},
		}},
		// Domain and ip_cidr items of one default rule are OR-ed, so "in the
		// fake-ip range AND no domain" needs a logical rule. domain_regex "."
		// matches any known domain (sniffed, reverse-mapped or requested);
		// inverted it matches connections without one.
		option.Rule{Type: C.RuleTypeLogical, LogicalOptions: option.LogicalRule{
			RawLogicalRule: option.RawLogicalRule{Mode: C.LogicalTypeAnd, Rules: []option.Rule{
				{Type: C.RuleTypeDefault, DefaultOptions: option.DefaultRule{RawDefaultRule: option.RawDefaultRule{
					Inbound: tun, IPCIDR: badoption.Listable[string]{FakeIPRange},
				}}},
				{Type: C.RuleTypeDefault, DefaultOptions: option.DefaultRule{RawDefaultRule: option.RawDefaultRule{
					DomainRegex: badoption.Listable[string]{"."}, Invert: true,
				}}},
			}},
			RuleAction: rejectAction(),
		}},
	)
}

// ensureDomainDestination returns the wrapper tag for a proxy route target,
// rendering the wrapper on first use.
func ensureDomainDestination(result *BuildResult, target string) string {
	tag := domainDestinationPrefix + target
	for _, outbound := range result.Options.Outbounds {
		if outbound.Tag == tag {
			return tag
		}
	}
	result.Options.Outbounds = append(result.Options.Outbounds, option.Outbound{Type: domaindest.Type, Tag: tag, Options: &domaindest.Options{
		Outbound: target, Inbounds: []string{TUNInboundTag},
	}})
	return tag
}

// addTUNDNS renders the DNS module that answers hijacked queries, and makes
// outbounds resolve domain destinations (node server names, direct
// connections) through the system resolver.
func addTUNDNS(result *BuildResult, final profile.RoutingAction) {
	result.Options.DNS = &option.DNSOptions{RawDNSOptions: option.RawDNSOptions{
		Servers: []option.DNSServerOptions{
			// sing-box's local transport is TUN-aware: it asks the physical
			// side for its resolvers (systemd-resolved link DNS of the default
			// interface on Linux, non-tunnel adapters on Windows, DHCP on
			// Darwin when a TUN exists) and, with auto_detect_interface, dials
			// them bound to that interface, so it never loops into the TUN.
			{Type: C.DNSTypeLocal, Tag: DNSLocalTag, Options: &option.LocalDNSServerOptions{}},
			{Type: C.DNSTypeTLS, Tag: DNSRemoteTag, Options: &option.RemoteTLSDNSServerOptions{RemoteDNSServerOptions: option.RemoteDNSServerOptions{
				RawLocalDNSServerOptions: option.RawLocalDNSServerOptions{DialerOptions: option.DialerOptions{Detour: selectedOutboundTag}},
				DNSServerAddressOptions:  option.DNSServerAddressOptions{Server: RemoteDNSServer},
			}}},
		},
		Rules:          mirrorDNSRules(result.Options.Route.Rules),
		Final:          DNSRemoteTag,
		ReverseMapping: true,
	}}
	if final.Type == "direct" {
		result.Options.DNS.Final = DNSLocalTag
	}
	result.Options.Route.DefaultDomainResolver = &option.DomainResolveOptions{Server: DNSLocalTag}
}

// mirrorDNSRules turns every inbound-independent route rule that matches
// domains into a DNS rule with the same domain matchers. Only the domain part
// is mirrored (port, network and CIDR conditions are unknown when a name is
// resolved), and order is kept, so the first route rule naming a domain
// decides where it resolves.
func mirrorDNSRules(rules []option.Rule) []option.DNSRule {
	var out []option.DNSRule
	for _, rule := range rules {
		if rule.Type != C.RuleTypeDefault {
			continue
		}
		raw := rule.DefaultOptions.RawDefaultRule
		if len(raw.Inbound) > 0 || len(raw.Domain)+len(raw.DomainSuffix) == 0 {
			continue
		}
		var action option.DNSRuleAction
		switch rule.DefaultOptions.RuleAction.Action {
		case C.RuleActionTypeRoute:
			server := DNSRemoteTag
			if rule.DefaultOptions.RuleAction.RouteOptions.Outbound == "direct" {
				server = DNSLocalTag
			}
			action = option.DNSRuleAction{Action: C.RuleActionTypeRoute, RouteOptions: option.DNSRouteActionOptions{Server: server}}
		case C.RuleActionTypeReject:
			action = option.DNSRuleAction{Action: C.RuleActionTypeReject, RejectOptions: option.RejectActionOptions{Method: C.RuleActionRejectMethodDefault}}
		default:
			continue
		}
		out = append(out, option.DNSRule{Type: C.RuleTypeDefault, DefaultOptions: option.DefaultDNSRule{
			RawDefaultDNSRule: option.RawDefaultDNSRule{
				Domain:       append(badoption.Listable[string](nil), raw.Domain...),
				DomainSuffix: append(badoption.Listable[string](nil), raw.DomainSuffix...),
			},
			DNSRuleAction: action,
		}})
	}
	return out
}
