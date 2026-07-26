// Package routing compiles Profile v2 routing into an immutable, allocation
// bounded classifier shared by packet and flow platform adapters.
package routing

import (
	"fmt"
	"net/netip"
	"strings"
	"time"

	"github.com/peakpassvpn/ppvpn-core/profile"
)

type Entry string

const (
	EntryTransparent    Entry = "transparent"
	EntryPlatformSafety Entry = "platform_safety"
	EntryLocalProxy     Entry = "local_proxy"
)

type Flow struct {
	Entry            Entry  `json:"entry"`
	LocalProxyNodeID string `json:"local_proxy_node_id,omitempty"`
	Hostname         string `json:"hostname,omitempty"`
	DestinationIP    string `json:"destination_ip,omitempty"`
	DestinationPort  uint16 `json:"destination_port"`
	Protocol         string `json:"protocol"`
}

type Decision struct {
	Type          string `json:"type"`
	Target        string `json:"target,omitempty"`
	NodeID        string `json:"node_id,omitempty"`
	RuleID        string `json:"rule_id,omitempty"`
	Priority      string `json:"priority"`
	Snapshot      uint64 `json:"snapshot,omitempty"`
	Authorization string `json:"authorization,omitempty"`
}

type Classifier struct {
	rules []compiledRule
	final profile.RoutingAction
	nodes map[string]struct{}
}

type compiledRule struct {
	id             string
	exact          map[string]struct{}
	suffixes       []string
	cidrs          []netip.Prefix
	ipPrivate      bool
	protocols      map[string]struct{}
	ports          map[uint16]struct{}
	portRanges     []portRange
	action         profile.RoutingAction
	hasDomainMatch bool
}

type portRange struct {
	start uint16
	end   uint16
}

func Compile(p *profile.Profile, now time.Time) (*Classifier, error) {
	if err := profile.Validate(p, now); err != nil {
		return nil, err
	}
	c := &Classifier{
		rules: make([]compiledRule, 0, len(p.Routing.Rules)),
		final: p.Routing.Final,
		nodes: make(map[string]struct{}, len(p.Nodes)),
	}
	for _, node := range p.Nodes {
		c.nodes[node.ID] = struct{}{}
	}
	for _, rule := range p.Routing.Rules {
		compiled, err := compileRule(rule)
		if err != nil {
			return nil, fmt.Errorf("compile rule %q: %w", rule.ID, err)
		}
		c.rules = append(c.rules, compiled)
	}
	return c, nil
}

func compileRule(rule profile.RoutingRule) (compiledRule, error) {
	out := compiledRule{
		id:             rule.ID,
		exact:          make(map[string]struct{}, len(rule.Match.Domains)),
		suffixes:       make([]string, 0, len(rule.Match.DomainSuffixes)),
		cidrs:          make([]netip.Prefix, 0, len(rule.Match.IPCIDRs)),
		ipPrivate:      rule.Match.IPIsPrivate,
		protocols:      make(map[string]struct{}, len(rule.Match.Protocols)),
		ports:          make(map[uint16]struct{}, len(rule.Match.Ports)),
		portRanges:     make([]portRange, 0, len(rule.Match.PortRanges)),
		action:         rule.Action,
		hasDomainMatch: len(rule.Match.Domains)+len(rule.Match.DomainSuffixes) > 0,
	}
	for _, domain := range rule.Match.Domains {
		normalized, ok := profile.NormalizeDomain(domain)
		if !ok {
			return compiledRule{}, fmt.Errorf("invalid exact domain")
		}
		out.exact[normalized] = struct{}{}
	}
	for _, suffix := range rule.Match.DomainSuffixes {
		normalized, ok := profile.NormalizeDomain(suffix)
		if !ok {
			return compiledRule{}, fmt.Errorf("invalid domain suffix")
		}
		out.suffixes = append(out.suffixes, normalized)
	}
	for _, cidr := range rule.Match.IPCIDRs {
		prefix, err := netip.ParsePrefix(cidr)
		if err != nil {
			return compiledRule{}, fmt.Errorf("invalid CIDR")
		}
		out.cidrs = append(out.cidrs, prefix.Masked())
	}
	for _, protocol := range rule.Match.Protocols {
		out.protocols[protocol] = struct{}{}
	}
	for _, port := range rule.Match.Ports {
		out.ports[port] = struct{}{}
	}
	for _, value := range rule.Match.PortRanges {
		start, end, err := profile.ParsePortRange(value)
		if err != nil {
			return compiledRule{}, err
		}
		out.portRanges = append(out.portRanges, portRange{start: start, end: end})
	}
	return out, nil
}

// Classify is a pure, non-blocking lookup over the compiled snapshot. selected
// is supplied by the runtime snapshot so a selected-node switch affects only
// subsequently classified flows.
func (c *Classifier) Classify(flow Flow, selected string) Decision {
	if flow.Entry == EntryPlatformSafety {
		return Decision{Type: "direct", Priority: "platform_safety"}
	}
	if flow.Entry == EntryLocalProxy {
		if _, ok := c.nodes[flow.LocalProxyNodeID]; ok {
			return Decision{
				Type:     "proxy",
				Target:   "node",
				NodeID:   flow.LocalProxyNodeID,
				Priority: "local_proxy",
			}
		}
		return Decision{Type: "reject", Priority: "local_proxy"}
	}
	for _, rule := range c.rules {
		if rule.matches(flow) {
			return resolve(rule.action, selected, rule.id, "profile")
		}
	}
	return resolve(c.final, selected, "", "final")
}

func (r compiledRule) matches(flow Flow) bool {
	if len(r.protocols) > 0 {
		if _, ok := r.protocols[strings.ToLower(flow.Protocol)]; !ok {
			return false
		}
	}
	if len(r.ports) > 0 || len(r.portRanges) > 0 {
		if _, ok := r.ports[flow.DestinationPort]; !ok {
			matched := false
			for _, value := range r.portRanges {
				if flow.DestinationPort >= value.start && flow.DestinationPort <= value.end {
					matched = true
					break
				}
			}
			if !matched {
				return false
			}
		}
	}
	hasAddressMatch := r.hasDomainMatch || len(r.cidrs) > 0 || r.ipPrivate
	addressMatched := false
	if r.hasDomainMatch {
		hostname, ok := profile.NormalizeDomain(flow.Hostname)
		if ok && r.matchesDomain(hostname) {
			addressMatched = true
		}
	}
	if len(r.cidrs) > 0 || r.ipPrivate {
		ip, err := netip.ParseAddr(flow.DestinationIP)
		if err == nil {
			addressMatched = addressMatched || r.ipPrivate && ip.IsPrivate()
			for _, prefix := range r.cidrs {
				addressMatched = addressMatched || prefix.Contains(ip)
			}
		}
	}
	return !hasAddressMatch || addressMatched
}

func (r compiledRule) matchesDomain(hostname string) bool {
	if _, ok := r.exact[hostname]; ok {
		return true
	}
	for _, suffix := range r.suffixes {
		if hostname == suffix || strings.HasSuffix(hostname, "."+suffix) {
			return true
		}
	}
	return false
}

func resolve(action profile.RoutingAction, selected, ruleID, priority string) Decision {
	decision := Decision{Type: action.Type, Target: action.Target, RuleID: ruleID, Priority: priority}
	if action.Type == "proxy" {
		if action.Target == "node" {
			decision.NodeID = action.NodeID
		} else {
			decision.NodeID = selected
		}
	}
	return decision
}
