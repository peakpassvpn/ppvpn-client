// Package config is the only boundary that knows sing-box option types.
package config

import (
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"net/netip"
	"strings"
	"time"

	"github.com/peakpassvpn/ppvpn-core/internal/failover"
	"github.com/peakpassvpn/ppvpn-core/internal/proxyinbound"
	"github.com/peakpassvpn/ppvpn-core/localproxy"
	"github.com/peakpassvpn/ppvpn-core/profile"
	C "github.com/sagernet/sing-box/constant"
	"github.com/sagernet/sing-box/option"
	"github.com/sagernet/sing/common/json/badoption"
)

type BuildResult struct {
	Options option.Options
	// NodeTags maps a logical node id to the outbound that represents it: the
	// ingress outbound itself for single-ingress nodes, or the failover group.
	NodeTags map[string]string
	// OutboundNodes maps every node and ingress outbound tag back to its
	// logical node id (used to attribute connections).
	OutboundNodes map[string]string
	// IngressKeys maps every outbound that is one ingress (a single-ingress
	// node tag, or a failover member tag) to the ingress endpoint_key.
	IngressKeys map[string]string

	// tun is set while building a TUN configuration: proxy route targets
	// then go through a domain-destination wrapper (see tundns.go).
	tun bool
}

const selectedOutboundTag = "selected"

func Build(p *profile.Profile, platform profile.PlatformCapabilities, now time.Time) (*BuildResult, error) {
	return BuildWithLocalProxies(p, platform, nil, now)
}

func BuildWithLocalProxies(p *profile.Profile, platform profile.PlatformCapabilities, proxies []localproxy.Endpoint, now time.Time) (*BuildResult, error) {
	return BuildWithOptions(p, platform, BuildOptions{LocalProxies: proxies}, now)
}

// BuildOptions carries the device-local inputs of a build.
type BuildOptions struct {
	LocalProxies []localproxy.Endpoint
	// RuleSets maps a profile rule set id to its verified local copy. A rule
	// set without an entry is unavailable: see addProfileRouting.
	RuleSets map[string]RuleSetFile
}

// RuleSetFile is a verified local copy of a profile rule set.
type RuleSetFile struct {
	// Path is the absolute path of the binary (.srs) file.
	Path string
	// MirrorDNS is true when the set matches domains and no destination IP
	// CIDRs. Only such sets are mirrored into DNS rules: a DNS rule whose
	// rule set carries CIDRs would resolve every name through that rule's
	// server first to test the answer.
	MirrorDNS bool
}

func BuildWithOptions(p *profile.Profile, platform profile.PlatformCapabilities, opts BuildOptions, now time.Time) (*BuildResult, error) {
	proxies := opts.LocalProxies
	if err := profile.Validate(p, now); err != nil {
		return nil, err
	}
	result := &BuildResult{NodeTags: make(map[string]string, len(p.Nodes)), OutboundNodes: map[string]string{}, IngressKeys: map[string]string{}}
	// sing-box logs are disabled at the dependency boundary because upstream
	// error messages are not guaranteed to preserve our credential policy.
	// Structured first-party runtime events remain available through WatchEvents.
	result.Options.Log = &option.LogOptions{Disabled: true, Level: normalizedLogLevel(platform.LogLevel), Timestamp: true}
	for i := range p.Nodes {
		outbounds, err := buildNode(result, p.Nodes[i])
		if err != nil {
			return nil, fmt.Errorf("build node %q: %w", p.Nodes[i].ID, err)
		}
		result.Options.Outbounds = append(result.Options.Outbounds, outbounds...)
	}
	selectedTags := make([]string, 0, len(p.Nodes))
	for _, node := range p.Nodes {
		selectedTags = append(selectedTags, result.NodeTags[node.ID])
	}
	selector := option.Outbound{Type: C.TypeSelector, Tag: selectedOutboundTag, Options: &option.SelectorOutboundOptions{
		Outbounds: selectedTags, Default: result.NodeTags[p.Selection.DefaultNodeID], InterruptExistConnections: false,
	}}
	result.Options.Outbounds = append([]option.Outbound{selector}, result.Options.Outbounds...)
	result.Options.Route = &option.RouteOptions{}
	if platform.TUN.Enabled {
		result.tun = true
		addTUNTrafficRules(result)
		addPlatformSafetyRules(result, p)
	}
	if len(proxies) > 0 {
		if err := addLocalProxies(result, proxies); err != nil {
			return nil, err
		}
	}
	if platform.TUN.Enabled {
		if err := addTUN(result, platform, ingressPrefixes(p)); err != nil {
			return nil, err
		}
	}
	if err := addProfileRouting(result, p.Routing, opts.RuleSets); err != nil {
		return nil, err
	}
	if platform.TUN.Enabled {
		addTUNDNS(result, p.Routing.Final, dnsRuleSetTags(opts.RuleSets))
	}
	return result, nil
}

func addPlatformSafetyRules(result *BuildResult, p *profile.Profile) {
	ensureDirectOutbound(result)
	// Every ingress (primary and backups) must bypass the tunnel, otherwise
	// failing over to a backup would route its handshake back into the TUN.
	seenDomain, seenIP := map[string]bool{}, map[string]bool{}
	for _, node := range p.Nodes {
		for _, ingress := range node.Ingresses {
			if domain, ok := profile.NormalizeDomain(ingress.Endpoint.Domain); ok && !seenDomain[domain] {
				seenDomain[domain] = true
				result.Options.Route.Rules = append(result.Options.Route.Rules, routeRule(
					option.RawDefaultRule{Domain: badoption.Listable[string]{domain}},
					"direct",
				))
			}
			if ingress.Endpoint.IP == "" {
				continue
			}
			ip := netip.MustParseAddr(ingress.Endpoint.IP).String()
			if seenIP[ip] {
				continue
			}
			seenIP[ip] = true
			result.Options.Route.Rules = append(result.Options.Route.Rules, routeRule(
				option.RawDefaultRule{IPCIDR: badoption.Listable[string]{ip}},
				"direct",
			))
		}
	}
}

// LocalProxyInboundTag is the shared authenticated local proxy inbound.
const LocalProxyInboundTag = "local-proxy"

// addLocalProxies renders one shared loopback inbound for every node. The
// inbound accepts exactly the per-node usernames, and one auth_user rule per
// node pins that user's traffic to the node outbound (the failover group for
// multi-ingress nodes). A final inbound rule rejects anything else, so local
// proxy traffic can never fall through to profile rules or the selected node.
func addLocalProxies(result *BuildResult, proxies []localproxy.Endpoint) error {
	first := proxies[0]
	prefix, _, ok := localproxy.ParseUsername(first.Username)
	if !ok || first.Listen != localproxy.Listen || first.Port == 0 || first.Password == "" {
		return fmt.Errorf("invalid local proxy endpoint for node %q", first.NodeID)
	}
	users := make([]proxyinbound.User, 0, len(proxies))
	for _, endpoint := range proxies {
		nodeOutbound, ok := result.NodeTags[endpoint.NodeID]
		if !ok {
			return fmt.Errorf("local proxy node %q does not exist", endpoint.NodeID)
		}
		if endpoint.Listen != first.Listen || endpoint.Port != first.Port || endpoint.Password != first.Password ||
			endpoint.Username != localproxy.FormatUsername(prefix, endpoint.NodeID) {
			return fmt.Errorf("invalid local proxy endpoint for node %q", endpoint.NodeID)
		}
		users = append(users, proxyinbound.User{Username: endpoint.Username, Password: endpoint.Password})
		result.Options.Route.Rules = append(result.Options.Route.Rules, routeRule(
			option.RawDefaultRule{Inbound: badoption.Listable[string]{LocalProxyInboundTag}, AuthUser: badoption.Listable[string]{endpoint.Username}},
			nodeOutbound,
		))
	}
	result.Options.Route.Rules = append(result.Options.Route.Rules, option.Rule{
		Type: C.RuleTypeDefault,
		DefaultOptions: option.DefaultRule{
			RawDefaultRule: option.RawDefaultRule{Inbound: badoption.Listable[string]{LocalProxyInboundTag}},
			RuleAction:     rejectAction(),
		},
	})
	loopback := badoption.Addr(netip.MustParseAddr(localproxy.Listen))
	result.Options.Inbounds = append(result.Options.Inbounds, option.Inbound{Type: proxyinbound.Type, Tag: LocalProxyInboundTag, Options: &proxyinbound.Options{
		ListenOptions: option.ListenOptions{Listen: &loopback, ListenPort: first.Port},
		Users:         users,
	}})
	return nil
}

// ingressPrefixes returns a host prefix for every ingress IP of every node.
func ingressPrefixes(p *profile.Profile) []netip.Prefix {
	seen := map[netip.Addr]bool{}
	var out []netip.Prefix
	for _, node := range p.Nodes {
		for _, ingress := range node.Ingresses {
			if ingress.Endpoint.IP == "" {
				continue
			}
			ip := netip.MustParseAddr(ingress.Endpoint.IP).Unmap()
			if !seen[ip] {
				seen[ip] = true
				out = append(out, netip.PrefixFrom(ip, ip.BitLen()))
			}
		}
	}
	return out
}

// Linux policy-routing namespace for the desktop TUN. sing-tun defaults to
// iproute2 table 2022 and rule priorities 9000..9010, and so does every other
// sing-tun based app (mihomo/Clash Meta, Clash Verge, plain sing-box, ...).
// sing-tun's cleanup (run both before installing rules and on Close) deletes
// every rule whose priority is in [ruleIndex, ruleIndex+10], whoever owns it,
// so sharing the defaults let a failed start of ours wipe mihomo's rules and
// leave its default route in table 2022 unreachable. Our own table and the
// disjoint priority range [9091, 9101] keep the two installs apart. The values
// avoid the defaults above, Tailscale (table 52, 5210..5270), wg-quick
// (table/fwmark 51820, 32764..32765) and the kernel's 0/32766/32767 rules.
const (
	tunIPRoute2TableIndex = 2091
	tunIPRoute2RuleIndex  = 9091
)

func addTUN(result *BuildResult, platform profile.PlatformCapabilities, excluded []netip.Prefix) error {
	stack := platform.TUN.Stack
	if stack == "" {
		stack = "mixed"
	}
	switch stack {
	case "mixed", "system", "gvisor":
	default:
		return fmt.Errorf("unsupported TUN stack %q", stack)
	}
	autoRoute := platform.Platform != "ios" && platform.Platform != "android"
	if autoRoute {
		// Desktop TUN owns the default route; bind the core's own outbound
		// sockets (ingress handshakes, direct traffic) to the physical
		// interface so they cannot loop back into the tunnel.
		result.Options.Route.AutoDetectInterface = true
	}
	options := &option.TunInboundOptions{Address: badoption.Listable[netip.Prefix]{netip.MustParsePrefix("172.19.0.1/30")}, Stack: stack, AutoRoute: autoRoute, StrictRoute: autoRoute}
	if autoRoute {
		// Keep every ingress IP (primary and backups) out of the tunnel at the
		// OS routing level too, so handshakes and ICMP/TCP probes from any
		// process (including an unprivileged sibling core) never loop.
		options.RouteExcludeAddress = excluded
		options.IPRoute2TableIndex = tunIPRoute2TableIndex
		options.IPRoute2RuleIndex = tunIPRoute2RuleIndex
	}
	result.Options.Inbounds = append(result.Options.Inbounds, option.Inbound{Type: C.TypeTun, Tag: TUNInboundTag, Options: options})
	return nil
}

// RuleSetTag is the sing-box tag of a profile rule set.
func RuleSetTag(id string) string { return "rule-set-" + id }

// addProfileRouting renders the profile rules. A rule set is referenced only
// when it has a verified local copy (files); otherwise it is unavailable and
// dropped from every rule naming it. A rule that loses all of its address
// matchers that way is dropped entirely, so it can never widen to "match
// everything" (or everything on its ports).
func addProfileRouting(result *BuildResult, routing profile.Routing, files map[string]RuleSetFile) error {
	used := map[string]bool{}
	for _, rule := range routing.Rules {
		var tags []string
		for _, id := range rule.Match.RuleSetIDs {
			if _, ok := files[id]; ok {
				tags = append(tags, RuleSetTag(id))
				used[id] = true
			}
		}
		if len(rule.Match.RuleSetIDs) > 0 && len(tags) == 0 && !rule.Match.HasAddressMatch() {
			continue
		}
		raw, err := buildRuleMatch(rule.Match)
		if err != nil {
			return fmt.Errorf("build routing rule %q: %w", rule.ID, err)
		}
		raw.RuleSet = tags
		action, err := buildRuleAction(result, rule.Action)
		if err != nil {
			return fmt.Errorf("build routing rule %q: %w", rule.ID, err)
		}
		result.Options.Route.Rules = append(result.Options.Route.Rules, option.Rule{
			Type: C.RuleTypeDefault,
			DefaultOptions: option.DefaultRule{
				RawDefaultRule: raw,
				RuleAction:     action,
			},
		})
	}
	for _, set := range routing.RuleSets {
		if used[set.ID] {
			result.Options.Route.RuleSet = append(result.Options.Route.RuleSet, option.RuleSet{
				Type: C.RuleSetTypeLocal, Tag: RuleSetTag(set.ID), Format: C.RuleSetFormatBinary,
				LocalOptions: option.LocalRuleSet{Path: files[set.ID].Path},
			})
		}
	}
	switch routing.Final.Type {
	case "direct":
		ensureDirectOutbound(result)
		result.Options.Route.Final = "direct"
	case "proxy":
		target, err := proxyTarget(result, routing.Final)
		if err != nil {
			return fmt.Errorf("build final routing action: %w", err)
		}
		result.Options.Route.Final = target
	case "reject":
		result.Options.Route.Rules = append(result.Options.Route.Rules, option.Rule{
			Type: C.RuleTypeDefault,
			DefaultOptions: option.DefaultRule{
				RuleAction: rejectAction(),
			},
		})
	default:
		return fmt.Errorf("unsupported final routing action")
	}
	return nil
}

func buildRuleMatch(match profile.RoutingMatch) (option.RawDefaultRule, error) {
	raw := option.RawDefaultRule{
		Network: badoption.Listable[string](append([]string(nil), match.Protocols...)),
		Port:    badoption.Listable[uint16](append([]uint16(nil), match.Ports...)),
	}
	for _, value := range match.Domains {
		normalized, ok := profile.NormalizeDomain(value)
		if !ok {
			return option.RawDefaultRule{}, fmt.Errorf("invalid exact domain")
		}
		raw.Domain = append(raw.Domain, normalized)
	}
	for _, value := range match.DomainSuffixes {
		normalized, ok := profile.NormalizeDomain(value)
		if !ok {
			return option.RawDefaultRule{}, fmt.Errorf("invalid domain suffix")
		}
		raw.DomainSuffix = append(raw.DomainSuffix, "."+normalized)
		raw.Domain = append(raw.Domain, normalized)
	}
	for _, value := range match.IPCIDRs {
		prefix, err := netip.ParsePrefix(value)
		if err != nil {
			return option.RawDefaultRule{}, fmt.Errorf("invalid CIDR")
		}
		raw.IPCIDR = append(raw.IPCIDR, prefix.Masked().String())
	}
	if match.IPIsPrivate {
		raw.IPCIDR = append(raw.IPCIDR,
			"10.0.0.0/8",
			"172.16.0.0/12",
			"192.168.0.0/16",
			"fc00::/7",
		)
	}
	for _, value := range match.PortRanges {
		start, end, err := profile.ParsePortRange(value)
		if err != nil {
			return option.RawDefaultRule{}, err
		}
		raw.PortRange = append(raw.PortRange, fmt.Sprintf("%d:%d", start, end))
	}
	return raw, nil
}

func buildRuleAction(result *BuildResult, action profile.RoutingAction) (option.RuleAction, error) {
	switch action.Type {
	case "direct":
		ensureDirectOutbound(result)
		return option.RuleAction{
			Action:       C.RuleActionTypeRoute,
			RouteOptions: option.RouteActionOptions{Outbound: "direct"},
		}, nil
	case "reject":
		return rejectAction(), nil
	case "proxy":
		target, err := proxyTarget(result, action)
		if err != nil {
			return option.RuleAction{}, err
		}
		return option.RuleAction{
			Action:       C.RuleActionTypeRoute,
			RouteOptions: option.RouteActionOptions{Outbound: target},
		}, nil
	default:
		return option.RuleAction{}, fmt.Errorf("unsupported routing action")
	}
}

func proxyTarget(result *BuildResult, action profile.RoutingAction) (string, error) {
	target := selectedOutboundTag
	if action.Target != "selected" {
		var ok bool
		target, ok = result.NodeTags[action.NodeID]
		if !ok || action.Target != "node" {
			return "", fmt.Errorf("fixed proxy node does not exist")
		}
	}
	if result.tun {
		return ensureDomainDestination(result, target), nil
	}
	return target, nil
}

func ensureDirectOutbound(result *BuildResult) {
	for _, outbound := range result.Options.Outbounds {
		if outbound.Tag == "direct" {
			return
		}
	}
	result.Options.Outbounds = append(result.Options.Outbounds, option.Outbound{
		Type:    C.TypeDirect,
		Tag:     "direct",
		Options: &option.DirectOutboundOptions{},
	})
}

// rejectAction renders a reject action. The method must be explicit: option
// structs handed to box.New skip JSON decoding, which is what normally turns
// an empty method into "default", and sing-box 1.13 panics ("unknown reject
// method") the first time a rule with an empty method matches.
func rejectAction() option.RuleAction {
	return option.RuleAction{Action: C.RuleActionTypeReject, RejectOptions: option.RejectActionOptions{Method: C.RuleActionRejectMethodDefault}}
}

func routeRule(raw option.RawDefaultRule, outbound string) option.Rule {
	return option.Rule{
		Type: C.RuleTypeDefault,
		DefaultOptions: option.DefaultRule{
			RawDefaultRule: raw,
			RuleAction: option.RuleAction{
				Action:       C.RuleActionTypeRoute,
				RouteOptions: option.RouteActionOptions{Outbound: outbound},
			},
		},
	}
}

// buildNode renders one logical node. A single-ingress node is the ingress
// outbound itself (tagged with the node tag); a multi-ingress node renders one
// outbound per ingress plus a failover group carrying the node tag.
func buildNode(result *BuildResult, n profile.Node) ([]option.Outbound, error) {
	tag := nodeTag(n.ID)
	result.NodeTags[n.ID] = tag
	result.OutboundNodes[tag] = n.ID
	if len(n.Ingresses) == 1 {
		out, err := buildOutbound(n.Ingresses[0], tag)
		if err != nil {
			return nil, err
		}
		result.IngressKeys[tag] = n.Ingresses[0].EndpointKey
		return []option.Outbound{out}, nil
	}
	outbounds := make([]option.Outbound, 0, len(n.Ingresses)+1)
	members := make([]string, 0, len(n.Ingresses))
	for i, ingress := range n.Ingresses {
		ingressTag := ingressTag(n.ID, ingress.EndpointKey)
		if _, dup := result.OutboundNodes[ingressTag]; dup {
			return nil, fmt.Errorf("ingress %d: outbound tag collision", i)
		}
		out, err := buildOutbound(ingress, ingressTag)
		if err != nil {
			return nil, fmt.Errorf("ingress %d: %w", i, err)
		}
		result.OutboundNodes[ingressTag] = n.ID
		result.IngressKeys[ingressTag] = ingress.EndpointKey
		members = append(members, ingressTag)
		outbounds = append(outbounds, out)
	}
	group := option.Outbound{Type: failover.Type, Tag: tag, Options: &failover.Options{Outbounds: members}}
	return append([]option.Outbound{group}, outbounds...), nil
}

func buildOutbound(n profile.Ingress, tag string) (option.Outbound, error) {
	server := option.ServerOptions{Server: n.Endpoint.Domain, ServerPort: n.Endpoint.Port}
	switch n.Protocol {
	case profile.ProtocolShadowsocks:
		c := n.Credentials.Shadowsocks
		// SIP022 EIH password: server iPSKs outermost first, then the user uPSK.
		keys := append(append([]string(nil), c.IdentityKeys...), c.UserKey)
		return option.Outbound{Type: C.TypeShadowsocks, Tag: tag, Options: &option.ShadowsocksOutboundOptions{ServerOptions: server, Method: c.Method, Password: strings.Join(keys, ":")}}, nil
	case profile.ProtocolVLESS:
		c := n.Credentials.VLESS
		return option.Outbound{Type: C.TypeVLESS, Tag: tag, Options: &option.VLESSOutboundOptions{ServerOptions: server, UUID: c.UUID, Flow: c.Flow, OutboundTLSOptionsContainer: option.OutboundTLSOptionsContainer{TLS: buildTLS(n.TLS)}}}, nil
	case profile.ProtocolAnyTLS:
		return option.Outbound{Type: C.TypeAnyTLS, Tag: tag, Options: &option.AnyTLSOutboundOptions{ServerOptions: server, Password: n.Credentials.AnyTLS.Password, OutboundTLSOptionsContainer: option.OutboundTLSOptionsContainer{TLS: buildTLS(n.TLS)}}}, nil
	default:
		return option.Outbound{}, fmt.Errorf("unsupported protocol")
	}
}

func buildTLS(t *profile.TLS) *option.OutboundTLSOptions {
	if t == nil {
		return nil
	}
	o := &option.OutboundTLSOptions{Enabled: true, ServerName: t.ServerName, Insecure: t.Insecure, ALPN: badoption.Listable[string](t.ALPN)}
	if t.Reality != nil {
		o.Reality = &option.OutboundRealityOptions{Enabled: true, PublicKey: t.Reality.PublicKey, ShortID: t.Reality.ShortID}
		// sing-box's REALITY client is implemented on uTLS: it must be enabled
		// here and the binary must be built with the with_utls tag.
		o.UTLS = &option.OutboundUTLSOptions{Enabled: true, Fingerprint: "chrome"}
	}
	return o
}

func nodeTag(id string) string {
	sum := sha256.Sum256([]byte(id))
	return "node-" + hex.EncodeToString(sum[:8])
}

// ingressTag derives a failover member tag from the node id and the replica's
// endpoint_key, so a replica keeps its outbound tag across profile revisions
// regardless of its position in the failover order.
func ingressTag(nodeID, endpointKey string) string {
	sum := sha256.Sum256([]byte(endpointKey))
	return nodeTag(nodeID) + "-" + hex.EncodeToString(sum[:4])
}
func normalizedLogLevel(v string) string {
	switch v {
	case "trace", "debug", "info", "warn", "error", "fatal", "panic":
		return v
	default:
		return "info"
	}
}

// SystemProxyInboundTag is the opt-in unauthenticated loopback HTTP/SOCKS5
// listener used for OS proxy settings.
const SystemProxyInboundTag = "system-proxy"

// SystemProxyInbound renders the system proxy listener. It is a stock mixed
// inbound without users (OS proxy settings cannot carry credentials), bound
// to 127.0.0.1 only. It has no route rules of its own, so its traffic takes
// the same path as TUN traffic: profile rules, then the selected node.
func SystemProxyInbound(port uint16) option.Inbound {
	loopback := badoption.Addr(netip.MustParseAddr(localproxy.Listen))
	return option.Inbound{Type: C.TypeMixed, Tag: SystemProxyInboundTag, Options: &option.HTTPMixedInboundOptions{
		ListenOptions: option.ListenOptions{Listen: &loopback, ListenPort: port},
	}}
}

// WithSystemProxy returns a copy of result whose inbounds include the system
// proxy on port, or exclude it when port is 0. result is not modified.
func WithSystemProxy(result *BuildResult, port uint16) *BuildResult {
	next := *result
	next.Options.Inbounds = make([]option.Inbound, 0, len(result.Options.Inbounds)+1)
	for _, inbound := range result.Options.Inbounds {
		if inbound.Tag != SystemProxyInboundTag {
			next.Options.Inbounds = append(next.Options.Inbounds, inbound)
		}
	}
	if port != 0 {
		next.Options.Inbounds = append(next.Options.Inbounds, SystemProxyInbound(port))
	}
	return &next
}
