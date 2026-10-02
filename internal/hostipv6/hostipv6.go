// Package hostipv6 reports whether the host can give the desktop TUN an IPv6
// address (Available), and whether it can reach the IPv6 internet without the
// tunnel (Route).
//
// sing-tun fails the whole TUN start when it cannot add the tunnel's IPv6
// address (Linux netlink EACCES under disable_ipv6, Windows
// SetIPAddressesForFamily with IPv6 disabled), so the core must leave IPv6 out
// on such hosts. Doing so never leaks: a host that cannot configure IPv6 on a
// new interface has no IPv6 path around the tunnel either.
package hostipv6

import "net/netip"

// Available reports whether IPv6 is enabled on this host. The result is not
// cached: an administrator can toggle IPv6 between two starts.
func Available() bool { return available() }

// Route reports whether the host has an IPv6 path to the internet of its
// own: one interface with both a global unicast IPv6 address (2000::/3) and
// an IPv6 default route. The desktop TUN only ever has a ULA, so it never
// counts. When the state cannot be read, Route returns true (IPv6 used as
// before) with the error, which the caller logs.
func Route() (bool, error) { return route() }

// globalUnicast reports whether a is a global unicast IPv6 address
// (2000::/3), the only kind that reaches the IPv6 internet.
func globalUnicast(a netip.Addr) bool {
	a = a.Unmap()
	return a.Is6() && a.As16()[0]&0xe0 == 0x20
}
