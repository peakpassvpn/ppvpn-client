// Package hostipv6 reports whether the host can give the desktop TUN an IPv6
// address.
//
// sing-tun fails the whole TUN start when it cannot add the tunnel's IPv6
// address (Linux netlink EACCES under disable_ipv6, Windows
// SetIPAddressesForFamily with IPv6 disabled), so the core must leave IPv6 out
// on such hosts. Doing so never leaks: a host that cannot configure IPv6 on a
// new interface has no IPv6 path around the tunnel either.
package hostipv6

// Available reports whether IPv6 is enabled on this host. The result is not
// cached: an administrator can toggle IPv6 between two starts.
func Available() bool { return available() }
