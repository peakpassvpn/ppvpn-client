package config

import (
	"fmt"
	"net/netip"
	"strings"

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
//     DNS module. The TUN announces its peer addresses (172.19.0.2 and, on
//     desktop, fdfe:dcba:9876::2) as the interface DNS servers where sing-tun
//     configures DNS, and desktop routes both IPv4 and IPv6 into the TUN, so
//     OS queries to any routed resolver are answered by the core.
//   - route rule 3 rejects a TUN connection to 198.18.0.0/15 whose domain is
//     unknown: that is a LAN fake-ip answer no node can reach, and proxying it
//     would hang for the node's whole connect timeout.
//   - DNS rules mirror the route rules: a domain routed direct resolves
//     through the system resolver dialed direct ("dns-local"); a domain routed
//     to a proxy resolves through DoT to 1.1.1.1 dialed through the selected
//     node ("dns-remote"); a rejected domain is refused. dns.final follows
//     route.final. Domain rule sets referenced by a route rule are mirrored
//     the same way; rule sets carrying IP CIDRs never affect DNS.
//     reverse_mapping remembers which domain each answered address belongs
//     to.
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
	// knownDomainRegex matches a domain name but not an IP literal. The HTTP
	// sniffer copies the Host header as is, so plain HTTP to an address
	// (curl http://198.18.1.117/) "sniffs" the address itself: an IPv4
	// literal has only digits and dots, an IPv6 literal has colons, and a
	// domain name always has another character (its TLD is never numeric)
	// and never a colon.
	knownDomainRegex = `^[^:]*[^0-9.:][^:]*$`
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
		// fake-ip range AND no domain" needs a logical rule. knownDomainRegex
		// matches a known domain name (sniffed, reverse-mapped or requested);
		// inverted it matches connections without one.
		option.Rule{Type: C.RuleTypeLogical, LogicalOptions: option.LogicalRule{
			RawLogicalRule: option.RawLogicalRule{Mode: C.LogicalTypeAnd, Rules: []option.Rule{
				{Type: C.RuleTypeDefault, DefaultOptions: option.DefaultRule{RawDefaultRule: option.RawDefaultRule{
					Inbound: tun, IPCIDR: badoption.Listable[string]{FakeIPRange},
				}}},
				{Type: C.RuleTypeDefault, DefaultOptions: option.DefaultRule{RawDefaultRule: option.RawDefaultRule{
					DomainRegex: badoption.Listable[string]{knownDomainRegex}, Invert: true,
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
func addTUNDNS(result *BuildResult, platform profile.PlatformCapabilities, final profile.RoutingAction, dnsRuleSets map[string]bool) error {
	local, err := localDNSServerOptions(platform.TUN.LocalDNSServers)
	if err != nil {
		return err
	}
	result.Options.DNS = &option.DNSOptions{RawDNSOptions: option.RawDNSOptions{
		Servers: []option.DNSServerOptions{
			local,
			{Type: C.DNSTypeTLS, Tag: DNSRemoteTag, Options: &option.RemoteTLSDNSServerOptions{RemoteDNSServerOptions: option.RemoteDNSServerOptions{
				RawLocalDNSServerOptions: option.RawLocalDNSServerOptions{DialerOptions: option.DialerOptions{Detour: selectedOutboundTag}},
				DNSServerAddressOptions:  option.DNSServerAddressOptions{Server: RemoteDNSServer},
			}}},
		},
		Rules:          mirrorDNSRules(result.Options.Route.Rules, dnsRuleSets),
		Final:          DNSRemoteTag,
		ReverseMapping: true,
	}}
	if final.Type == "direct" {
		result.Options.DNS.Final = DNSLocalTag
	}
	result.Options.Route.DefaultDomainResolver = &option.DomainResolveOptions{Server: DNSLocalTag}
	return nil
}

// Tunnel prefixes: a resolver inside them is the core's own tunnel DNS (a
// stale system DNS entry left by the host), and querying it would loop.
var tunnelPrefixes = []netip.Prefix{netip.MustParsePrefix("172.19.0.0/30"), netip.MustParsePrefix("fdfe:dcba:9876::/126")}

// LocalDNSServer validates the host-supplied physical resolvers and returns
// the first one outside the tunnel. ok is false when none is left. Every
// entry must be an IP, IP:port or [IPv6]:port (zones allowed); the default
// port is 53.
func LocalDNSServer(entries []string) (server netip.AddrPort, ok bool, err error) {
	for _, entry := range entries {
		entry = strings.TrimSpace(entry)
		address, err := netip.ParseAddrPort(entry)
		if err != nil {
			ip, ipErr := netip.ParseAddr(entry)
			if ipErr != nil {
				return netip.AddrPort{}, false, fmt.Errorf("invalid local DNS server %q: want an IP, IP:port or [IPv6]:port", entry)
			}
			address = netip.AddrPortFrom(ip, 53)
		}
		if address.Port() == 0 || !address.Addr().IsValid() || address.Addr().IsUnspecified() {
			return netip.AddrPort{}, false, fmt.Errorf("invalid local DNS server %q", entry)
		}
		address = netip.AddrPortFrom(address.Addr().Unmap(), address.Port())
		if !ok && !inTunnel(address.Addr()) {
			server, ok = address, true
		}
	}
	return server, ok, nil
}

func inTunnel(ip netip.Addr) bool {
	for _, prefix := range tunnelPrefixes {
		if prefix.Contains(ip.WithZone("")) {
			return true
		}
	}
	return false
}

// localDNSServerOptions renders dns-local. With a host-supplied physical
// resolver it is a plain UDP server; auto_detect_interface binds its socket
// to the physical interface, so it never enters the tunnel. Without one it
// is sing-box's local transport: on Linux it asks systemd-resolved for the
// default interface's link DNS, on Windows it reads non-tunnel adapters, and
// on Darwin with a TUN it asks DHCP (desktop builds include with_dhcp) and
// otherwise falls back to the system resolver, which the desktop points at
// the tunnel, so Darwin hosts should pass their resolvers.
func localDNSServerOptions(entries []string) (option.DNSServerOptions, error) {
	server, ok, err := LocalDNSServer(entries)
	if err != nil {
		return option.DNSServerOptions{}, err
	}
	if !ok {
		return option.DNSServerOptions{Type: C.DNSTypeLocal, Tag: DNSLocalTag, Options: &option.LocalDNSServerOptions{}}, nil
	}
	return option.DNSServerOptions{Type: C.DNSTypeUDP, Tag: DNSLocalTag, Options: &option.RemoteDNSServerOptions{
		DNSServerAddressOptions: option.DNSServerAddressOptions{Server: server.Addr().String(), ServerPort: server.Port()},
	}}, nil
}

// dnsRuleSetTags returns the tags of the rule sets DNS rules may reference
// (see RuleSetFile.MirrorDNS).
func dnsRuleSetTags(files map[string]RuleSetFile) map[string]bool {
	tags := map[string]bool{}
	for id, file := range files {
		if file.MirrorDNS {
			tags[RuleSetTag(id)] = true
		}
	}
	return tags
}

// mirrorDNSRules turns every inbound-independent route rule that matches
// domains into a DNS rule with the same domain matchers. Only the domain part
// is mirrored (port, network and CIDR conditions are unknown when a name is
// resolved), and order is kept, so the first route rule naming a domain
// decides where it resolves. Domain rule sets (dnsRuleSets) count as domain
// matchers; rule sets carrying IP CIDRs are left out.
func mirrorDNSRules(rules []option.Rule, dnsRuleSets map[string]bool) []option.DNSRule {
	var out []option.DNSRule
	for _, rule := range rules {
		if rule.Type != C.RuleTypeDefault {
			continue
		}
		raw := rule.DefaultOptions.RawDefaultRule
		var ruleSets badoption.Listable[string]
		for _, tag := range raw.RuleSet {
			if dnsRuleSets[tag] {
				ruleSets = append(ruleSets, tag)
			}
		}
		if len(raw.Inbound) > 0 || len(raw.Domain)+len(raw.DomainSuffix)+len(ruleSets) == 0 {
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
				RuleSet:      ruleSets,
			},
			DNSRuleAction: action,
		}})
	}
	return out
}
