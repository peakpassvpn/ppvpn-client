package config

import (
	"fmt"
	"net/netip"
	"strings"

	"github.com/peakpassvpn/ppvpn-core/internal/domaindest"
	"github.com/peakpassvpn/ppvpn-core/internal/localdns"
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
//     DNS module. The TUN announces its peer addresses (10.60.159.90 and, on
//     desktop, fde2:ec40:9312:c7fd::2) as the interface DNS servers where sing-tun
//     configures DNS, and desktop routes both IPv4 and IPv6 into the TUN, so
//     OS queries to any routed resolver are answered by the core.
//   - route rule 3 rejects anything else sent to the tunnel's own networks
//     (it exists nowhere, and would otherwise go direct and hang).
//   - route rule 4 rejects a TUN connection to 198.18.0.0/15 whose domain is
//     unknown: that is a LAN fake-ip answer no node can reach, and proxying it
//     would hang for the node's whole connect timeout.
//   - route rule 5 is the client baseline: private, loopback, link-local,
//     multicast and broadcast destinations go direct, before profile rules
//     and in every routing mode.
//   - DNS rules mirror the route rules: a domain routed direct resolves
//     through the system resolver dialed direct ("dns-local"); a domain routed
//     to a proxy resolves through DoT to 1.1.1.1 dialed through the selected
//     node ("dns-remote", falling back to 8.8.8.8 and 9.9.9.9 the same way,
//     see dnstransport); a rejected domain is refused. dns.final follows
//     route.final. Domain rule sets referenced by a route rule are mirrored
//     the same way; rule sets carrying IP CIDRs never affect DNS.
//     reverse_mapping remembers which domain each answered address belongs
//     to.
//   - every proxy route target is wrapped by domaindest, which hands the known
//     domain (sniffed, or from DNS) to the node instead of the address.

// remoteDNSFallbacks are the DoT servers dns-remote falls back to, in order,
// each dialed through the selected node like dns-remote. Only public
// resolvers outside mainland China: these resolve the domains routed to a
// proxy, and a domestic resolver would log them and may answer them
// poisoned. When all fail the query fails.
var remoteDNSFallbacks = []string{"8.8.8.8", "9.9.9.9"}

// DNSRemoteFallbackTags are the tags of the remoteDNSFallbacks servers.
var DNSRemoteFallbackTags = func() []string {
	tags := make([]string, len(remoteDNSFallbacks))
	for i, server := range remoteDNSFallbacks {
		tags[i] = DNSRemoteTag + "-" + server
	}
	return tags
}()

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
		// Anything else to the tunnel's own networks exists nowhere: reject
		// it at once instead of sending it out the physical interface, where
		// the private baseline below would put it, to hang until timeout.
		option.Rule{Type: C.RuleTypeDefault, DefaultOptions: option.DefaultRule{
			RawDefaultRule: option.RawDefaultRule{Inbound: tun, IPCIDR: prefixStrings(tunPrefixes)},
			RuleAction:     rejectAction(),
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
	// Client baseline: private, loopback, link-local, multicast and broadcast
	// destinations (profile.PrivatePrefixes) always go direct from the
	// tunnel, before any profile rule and whatever the routing mode, so LAN
	// devices and discovery keep working even without a profile
	// bypass-private rule.
	ensureDirectOutbound(result)
	result.Options.Route.Rules = append(result.Options.Route.Rules, routeRule(
		option.RawDefaultRule{Inbound: tun, IPCIDR: prefixStrings(profile.PrivatePrefixes)},
		"direct",
	))
}

func prefixStrings(prefixes []netip.Prefix) badoption.Listable[string] {
	out := make(badoption.Listable[string], len(prefixes))
	for i, prefix := range prefixes {
		out[i] = prefix.String()
	}
	return out
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
	servers := []option.DNSServerOptions{local, remoteDNSServer(DNSRemoteTag, RemoteDNSServer)}
	for i, server := range remoteDNSFallbacks {
		servers = append(servers, remoteDNSServer(DNSRemoteFallbackTags[i], server))
	}
	result.Options.DNS = &option.DNSOptions{RawDNSOptions: option.RawDNSOptions{
		Servers:        servers,
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

// remoteDNSServer is DoT to server, dialed through the selected node.
func remoteDNSServer(tag, server string) option.DNSServerOptions {
	return option.DNSServerOptions{Type: C.DNSTypeTLS, Tag: tag, Options: &option.RemoteTLSDNSServerOptions{RemoteDNSServerOptions: option.RemoteDNSServerOptions{
		RawLocalDNSServerOptions: option.RawLocalDNSServerOptions{DialerOptions: option.DialerOptions{Detour: selectedOutboundTag}},
		DNSServerAddressOptions:  option.DNSServerAddressOptions{Server: server},
	}}}
}

// tunnelPrefixes: a resolver inside them is the core's own tunnel DNS (a
// stale system DNS entry left by the host, including one from before 0.5.7),
// and querying it would loop.
var tunnelPrefixes = append(append([]netip.Prefix(nil), tunPrefixes...), tunLegacyPrefixes...)

// LocalDNSServers validates the host-supplied physical resolvers and returns
// those outside the tunnel, in order. Every entry must be an IP, IP:port or
// [IPv6]:port (zones allowed); the default port is 53.
func LocalDNSServers(entries []string) ([]netip.AddrPort, error) {
	var servers []netip.AddrPort
	for _, entry := range entries {
		entry = strings.TrimSpace(entry)
		address, err := netip.ParseAddrPort(entry)
		if err != nil {
			ip, ipErr := netip.ParseAddr(entry)
			if ipErr != nil {
				return nil, fmt.Errorf("invalid local DNS server %q: want an IP, IP:port or [IPv6]:port", entry)
			}
			address = netip.AddrPortFrom(ip, 53)
		}
		if address.Port() == 0 || !address.Addr().IsValid() || address.Addr().IsUnspecified() {
			return nil, fmt.Errorf("invalid local DNS server %q", entry)
		}
		address = netip.AddrPortFrom(address.Addr().Unmap(), address.Port())
		if !inTunnel(address.Addr()) {
			servers = append(servers, address)
		}
	}
	return servers, nil
}

func inTunnel(ip netip.Addr) bool {
	for _, prefix := range tunnelPrefixes {
		if prefix.Contains(ip.WithZone("")) {
			return true
		}
	}
	return false
}

// ownLocalDNS reports whether this build reads the default interface's
// resolvers itself (localdns: Windows, macOS, lab builds); tests replace it.
var ownLocalDNS = localdns.Supported

// localDNSServerOptions renders dns-local. Host-supplied physical resolvers
// are used as they are, all of them in order (a static override: they do not
// follow network changes). Otherwise, on Windows and macOS it is the core's
// own transport (localdns), which asks the DNS servers of the default
// interface and reads them again on every interface change; elsewhere it is
// sing-box's local transport, which on Linux asks systemd-resolved for the
// default interface's link DNS or reads /etc/resolv.conf. Either way the
// socket is bound to the physical interface (auto_detect_interface), never
// the tunnel, and neither asks the system resolver the desktop points at the
// tunnel.
func localDNSServerOptions(entries []string) (option.DNSServerOptions, error) {
	servers, err := LocalDNSServers(entries)
	if err != nil {
		return option.DNSServerOptions{}, err
	}
	switch {
	case len(servers) > 0:
		return option.DNSServerOptions{Type: localdns.Type, Tag: DNSLocalTag, Options: &localdns.Options{Servers: servers}}, nil
	case ownLocalDNS():
		return option.DNSServerOptions{Type: localdns.Type, Tag: DNSLocalTag, Options: &localdns.Options{Exclude: tunnelPrefixes}}, nil
	default:
		return option.DNSServerOptions{Type: C.DNSTypeLocal, Tag: DNSLocalTag, Options: &option.LocalDNSServerOptions{}}, nil
	}
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
