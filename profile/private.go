package profile

import "net/netip"

// PrivatePrefixes are the destinations ip_is_private matches, aligned with
// the subscription side's system private bypass: RFC 1918, CGNAT (RFC 6598),
// "this network", loopback, link-local, multicast, reserved and limited
// broadcast for IPv4; ULA, link-local, multicast and loopback for IPv6. They
// never belong behind a proxy node. The desktop TUN routes the same ranges
// direct in its core baseline rule regardless of the profile.
var PrivatePrefixes = mustPrefixes(
	"10.0.0.0/8",
	"172.16.0.0/12",
	"192.168.0.0/16",
	"100.64.0.0/10",
	"0.0.0.0/8",
	"127.0.0.0/8",
	"169.254.0.0/16",
	"224.0.0.0/4",
	"240.0.0.0/4",
	"255.255.255.255/32",
	"fc00::/7",
	"fe80::/10",
	"ff00::/8",
	"::1/128",
)

// IsPrivateIP reports whether ip is in PrivatePrefixes (an IPv4-mapped IPv6
// address counts as its IPv4 address; the zone is ignored).
func IsPrivateIP(ip netip.Addr) bool {
	ip = ip.Unmap().WithZone("")
	for _, prefix := range PrivatePrefixes {
		if prefix.Contains(ip) {
			return true
		}
	}
	return false
}
