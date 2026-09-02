// Package config is the only boundary that knows sing-box option types.
package config

import (
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"net/netip"
	"strings"
	"time"

	"github.com/peakpassvpn/ppvpn-core/localproxy"
	"github.com/peakpassvpn/ppvpn-core/profile"
	C "github.com/sagernet/sing-box/constant"
	"github.com/sagernet/sing-box/option"
	"github.com/sagernet/sing/common/auth"
	"github.com/sagernet/sing/common/json/badoption"
)

type BuildResult struct {
	Options  option.Options
	NodeTags map[string]string
}

const selectedOutboundTag = "selected"

func Build(p *profile.Profile, platform profile.PlatformCapabilities, now time.Time) (*BuildResult, error) {
	return BuildWithLocalProxies(p, platform, nil, now)
}

func BuildWithLocalProxies(p *profile.Profile, platform profile.PlatformCapabilities, proxies []localproxy.Endpoint, now time.Time) (*BuildResult, error) {
	if err := profile.Validate(p, now); err != nil {
		return nil, err
	}
	result := &BuildResult{NodeTags: make(map[string]string, len(p.Nodes))}
	// sing-box logs are disabled at the dependency boundary because upstream
	// error messages are not guaranteed to preserve our credential policy.
	// Structured first-party runtime events remain available through WatchEvents.
	result.Options.Log = &option.LogOptions{Disabled: true, Level: normalizedLogLevel(platform.LogLevel), Timestamp: true}
	for i := range p.Nodes {
		n := &p.Nodes[i]
		tag := nodeTag(n.ID)
		result.NodeTags[n.ID] = tag
		out, err := buildOutbound(*n, tag)
		if err != nil {
			return nil, fmt.Errorf("build node %q: %w", n.ID, err)
		}
		result.Options.Outbounds = append(result.Options.Outbounds, out)
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
		addPlatformSafetyRules(result, p)
	}
	if len(proxies) > 0 {
		if err := addLocalProxies(result, proxies); err != nil {
			return nil, err
		}
	}
	if platform.TUN.Enabled {
		if err := addTUN(result, platform); err != nil {
			return nil, err
		}
	}
	if err := addProfileRouting(result, p.Routing); err != nil {
		return nil, err
	}
	return result, nil
}

func addPlatformSafetyRules(result *BuildResult, p *profile.Profile) {
	ensureDirectOutbound(result)
	for _, node := range p.Nodes {
		domain, ok := profile.NormalizeDomain(node.Endpoint.Domain)
		if ok {
			result.Options.Route.Rules = append(result.Options.Route.Rules, routeRule(
				option.RawDefaultRule{Domain: badoption.Listable[string]{domain}},
				"direct",
			))
		}
		result.Options.Route.Rules = append(result.Options.Route.Rules, routeRule(
			option.RawDefaultRule{IPCIDR: badoption.Listable[string]{netip.MustParseAddr(node.Endpoint.IP).String()}},
			"direct",
		))
	}
}

func addLocalProxies(result *BuildResult, proxies []localproxy.Endpoint) error {
	loopback := badoption.Addr(netip.MustParseAddr("127.0.0.1"))
	for _, endpoint := range proxies {
		nodeOutbound, ok := result.NodeTags[endpoint.NodeID]
		if !ok {
			return fmt.Errorf("local proxy node %q does not exist", endpoint.NodeID)
		}
		if endpoint.Listen != "127.0.0.1" || endpoint.Port == 0 || endpoint.Username == "" || endpoint.Password == "" {
			return fmt.Errorf("invalid local proxy endpoint for node %q", endpoint.NodeID)
		}
		inboundTag := "proxy-" + strings.TrimPrefix(nodeOutbound, "node-")
		result.Options.Inbounds = append(result.Options.Inbounds, option.Inbound{Type: C.TypeMixed, Tag: inboundTag, Options: &option.HTTPMixedInboundOptions{ListenOptions: option.ListenOptions{Listen: &loopback, ListenPort: endpoint.Port}, Users: []auth.User{{Username: endpoint.Username, Password: endpoint.Password}}}})
		result.Options.Route.Rules = append(result.Options.Route.Rules, routeRule(
			option.RawDefaultRule{Inbound: badoption.Listable[string]{inboundTag}},
			nodeOutbound,
		))
	}
	return nil
}

func addTUN(result *BuildResult, platform profile.PlatformCapabilities) error {
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
	result.Options.Inbounds = append(result.Options.Inbounds, option.Inbound{Type: C.TypeTun, Tag: "tun", Options: &option.TunInboundOptions{Address: badoption.Listable[netip.Prefix]{netip.MustParsePrefix("172.19.0.1/30")}, Stack: stack, AutoRoute: autoRoute, StrictRoute: autoRoute}})
	return nil
}

func addProfileRouting(result *BuildResult, routing profile.Routing) error {
	for _, rule := range routing.Rules {
		raw, err := buildRuleMatch(rule.Match)
		if err != nil {
			return fmt.Errorf("build routing rule %q: %w", rule.ID, err)
		}
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
				RuleAction: option.RuleAction{Action: C.RuleActionTypeReject},
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
		return option.RuleAction{Action: C.RuleActionTypeReject}, nil
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
	if action.Target == "selected" {
		return selectedOutboundTag, nil
	}
	target, ok := result.NodeTags[action.NodeID]
	if !ok || action.Target != "node" {
		return "", fmt.Errorf("fixed proxy node does not exist")
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

func buildOutbound(n profile.Node, tag string) (option.Outbound, error) {
	server := option.ServerOptions{Server: n.Endpoint.Domain, ServerPort: n.Endpoint.Port}
	switch n.Protocol {
	case profile.ProtocolShadowsocks:
		c := n.Credentials.Shadowsocks
		keys := append(append([]string(nil), c.IdentityKeys...), c.ServerKey)
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
	}
	return o
}

func nodeTag(id string) string {
	sum := sha256.Sum256([]byte(id))
	return "node-" + hex.EncodeToString(sum[:8])
}
func normalizedLogLevel(v string) string {
	switch v {
	case "trace", "debug", "info", "warn", "error", "fatal", "panic":
		return v
	default:
		return "info"
	}
}
