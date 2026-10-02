package localdns

import (
	"net/netip"
	"strconv"
	"strings"
)

// scutilScript is fed to scutil on macOS: the primary interfaces of IPv4 and
// IPv6, then the primary service's DNS (manual servers included). The
// desktop's own entry (State:/Network/Service/com.peakpassvpn.ppvpn.tun/DNS,
// a supplemental resolver pointing at the tunnel) is a separate service and
// not part of it; tunnel addresses are filtered anyway.
const scutilScript = "show State:/Network/Global/IPv4\nshow State:/Network/Global/IPv6\nshow State:/Network/Global/DNS\nquit\n"

// scutilDict is one top-level dictionary printed by `scutil` `show`: plain
// values and arrays (nested dictionaries are skipped).
type scutilDict struct {
	values map[string]string
	arrays map[string][]string
}

// parseScutilShows splits the output of a script of `show` commands into one
// result per command, in order: nil for "No such key".
func parseScutilShows(output string) []*scutilDict {
	var results []*scutilDict
	var current *scutilDict
	var array string
	depth := 0
	for _, raw := range strings.Split(output, "\n") {
		line := strings.TrimSpace(raw)
		switch {
		case current == nil && line == "No such key":
			results = append(results, nil)
		case current == nil && line == "<dictionary> {":
			current = &scutilDict{values: map[string]string{}, arrays: map[string][]string{}}
			depth = 1
		case current == nil:
		case line == "}":
			depth--
			array = ""
			if depth == 0 {
				results = append(results, current)
				current = nil
			}
		default:
			key, value, ok := strings.Cut(line, " : ")
			if !ok {
				continue
			}
			key, value = strings.TrimSpace(key), strings.TrimSpace(value)
			switch {
			case strings.HasSuffix(value, "{"):
				depth++
				if depth == 2 && value == "<array> {" {
					array = key
					current.arrays[key] = []string{}
				}
			case depth == 1:
				current.values[key] = value
			case depth == 2 && array != "":
				current.arrays[array] = append(current.arrays[array], value)
			}
		}
	}
	return results
}

// globalServers returns the primary service's DNS servers from the output of
// scutilScript when they belong to iface: by the entry's __IF_INDEX__ when
// configd recorded it, else when iface is the primary interface (IPv4's, or
// IPv6's on an IPv6-only network). ok is false otherwise: another VPN's
// service is primary (its utun, with its own DNS), or configd has not caught
// up with a network switch yet.
func globalServers(output string, iface string, index int) (servers []netip.AddrPort, ok bool) {
	shows := parseScutilShows(output)
	if len(shows) != 3 || shows[2] == nil {
		return nil, false
	}
	if owner := shows[2].values["__IF_INDEX__"]; owner != "" {
		if owner != strconv.Itoa(index) {
			return nil, false
		}
	} else {
		primary := ""
		for _, global := range shows[:2] {
			if global != nil && global.values["PrimaryInterface"] != "" {
				primary = global.values["PrimaryInterface"]
				break
			}
		}
		if primary != iface {
			return nil, false
		}
	}
	return parseAddresses(shows[2].arrays["ServerAddresses"]), true
}

// scopedServers returns the nameservers of the scoped resolver of interface
// index in the output of `scutil --dns` ("DNS configuration (for scoped
// queries)"): the resolvers configd keeps per interface, without a domain.
func scopedServers(output string, index int) []netip.AddrPort {
	_, scoped, ok := strings.Cut(output, "DNS configuration (for scoped queries)")
	if !ok {
		return nil
	}
	want := strconv.Itoa(index)
	for _, resolver := range strings.Split(scoped, "resolver #")[1:] {
		var names []string
		match, domain := false, false
		for _, raw := range strings.Split(resolver, "\n") {
			key, value, ok := strings.Cut(strings.TrimSpace(raw), " : ")
			if !ok {
				continue
			}
			key, value = strings.TrimSpace(key), strings.TrimSpace(value)
			switch {
			case strings.HasPrefix(key, "nameserver["):
				names = append(names, value)
			case key == "if_index":
				number, _, _ := strings.Cut(value, " ")
				match = number == want
			case key == "domain":
				domain = true
			}
		}
		if match && !domain {
			return parseAddresses(names)
		}
	}
	return nil
}

// parseAddresses parses resolver addresses (zones kept) on port 53; entries
// that are not addresses are skipped.
func parseAddresses(values []string) []netip.AddrPort {
	var servers []netip.AddrPort
	for _, value := range values {
		if addr, err := netip.ParseAddr(value); err == nil {
			servers = append(servers, netip.AddrPortFrom(addr, 53))
		}
	}
	return servers
}
