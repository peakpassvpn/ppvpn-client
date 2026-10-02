package hostipv6

import (
	"net"
	"net/netip"
	"syscall"

	rib "golang.org/x/net/route"
)

// Darwin keeps IPv6 enabled (it cannot be switched off system-wide).
func available() bool { return true }

func route() (bool, error) {
	data, err := rib.FetchRIB(syscall.AF_INET6, rib.RIBTypeRoute, 0)
	if err != nil {
		return true, err
	}
	messages, err := rib.ParseRIB(rib.RIBTypeRoute, data)
	if err != nil {
		return true, err
	}
	for _, index := range defaultRouteIndexes(messages) {
		iface, err := net.InterfaceByIndex(index)
		if err != nil || iface.Flags&net.FlagUp == 0 {
			continue
		}
		addrs, err := iface.Addrs()
		if err != nil {
			continue
		}
		for _, addr := range addrs {
			if prefix, err := netip.ParsePrefix(addr.String()); err == nil && globalUnicast(prefix.Addr()) {
				return true, nil
			}
		}
	}
	return false, nil
}

// defaultRouteIndexes lists the interfaces of the usable IPv6 default
// routes (::/0, up, neither reject nor blackhole). A TUN's split routes
// (::/1, 8000::/1, 100::/8...) are not default routes.
func defaultRouteIndexes(messages []rib.Message) []int {
	var out []int
	for _, message := range messages {
		m, ok := message.(*rib.RouteMessage)
		if !ok || m.Flags&syscall.RTF_UP == 0 || m.Flags&(syscall.RTF_REJECT|syscall.RTF_BLACKHOLE) != 0 {
			continue
		}
		if len(m.Addrs) <= syscall.RTAX_NETMASK || !zeroInet6(m.Addrs[syscall.RTAX_DST], false) || !zeroInet6(m.Addrs[syscall.RTAX_NETMASK], true) {
			continue
		}
		out = append(out, m.Index)
	}
	return out
}

// zeroInet6 reports whether a is the IPv6 unspecified address; a missing
// netmask (the kernel omits it for a default route) counts when nilOK.
func zeroInet6(a rib.Addr, nilOK bool) bool {
	if a == nil {
		return nilOK
	}
	inet6, ok := a.(*rib.Inet6Addr)
	return ok && inet6.IP == [16]byte{}
}
