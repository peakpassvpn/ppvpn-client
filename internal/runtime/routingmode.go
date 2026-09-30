package runtime

import (
	"errors"
	"slices"

	"github.com/peakpassvpn/ppvpn-core/profile"
)

// RoutingMode selects which profile rules apply. It is a host choice made per
// apply-profile, not part of the profile.
type RoutingMode string

const (
	// RoutingModeRules applies every profile rule and the profile's final
	// action (the default).
	RoutingModeRules RoutingMode = "rules"
	// RoutingModeGlobal keeps only the baseline rules, in profile order, and
	// sends everything else to the selected node. The core's own rules (TUN
	// sniffing, DNS hijack, fake-ip reject, ingress bypass) are unaffected.
	RoutingModeGlobal RoutingMode = "global"
)

// ErrRoutingModeInvalid rejects a routing mode other than rules or global.
var ErrRoutingModeInvalid = errors.New("routing mode must be rules or global")

// ParseRoutingMode reads a host-supplied mode; empty means rules.
func ParseRoutingMode(value string) (RoutingMode, error) {
	switch RoutingMode(value) {
	case "", RoutingModeRules:
		return RoutingModeRules, nil
	case RoutingModeGlobal:
		return RoutingModeGlobal, nil
	}
	return "", ErrRoutingModeInvalid
}

// effectiveProfile returns the profile the runtime builds from in mode. In
// the global mode it is a copy of p with only the baseline rules, the final
// action "proxy the selected node", and only the rule sets those rules
// reference, so dropped rules' rule sets are neither downloaded nor refreshed.
// p is never modified.
func effectiveProfile(p *profile.Profile, mode RoutingMode) (*profile.Profile, error) {
	if mode != RoutingModeGlobal {
		return p, nil
	}
	out, err := cloneProfile(p)
	if err != nil {
		return nil, err
	}
	var rules []profile.RoutingRule
	referenced := map[string]bool{}
	for _, rule := range out.Routing.Rules {
		if !rule.Baseline {
			continue
		}
		rules = append(rules, rule)
		for _, id := range rule.Match.RuleSetIDs {
			referenced[id] = true
		}
	}
	out.Routing.Rules = rules
	out.Routing.RuleSets = slices.DeleteFunc(out.Routing.RuleSets, func(set profile.RuleSet) bool { return !referenced[set.ID] })
	out.Routing.Final = profile.RoutingAction{Type: "proxy", Target: "selected"}
	return out, nil
}
