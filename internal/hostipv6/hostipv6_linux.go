package hostipv6

import (
	"encoding/hex"
	"errors"
	"io/fs"
	"net/netip"
	"os"
	"strconv"
	"strings"
)

// disableIPv6Files are the sysctls that decide whether a new interface (the
// TUN) gets IPv6: "all" overrides every interface and "default" is inherited
// by interfaces created later. Without /proc/sys/net/ipv6 at all the kernel
// runs with ipv6.disable=1.
var disableIPv6Files = []string{
	"/proc/sys/net/ipv6/conf/all/disable_ipv6",
	"/proc/sys/net/ipv6/conf/default/disable_ipv6",
}

func available() bool { return availableFrom(os.ReadFile) }

func availableFrom(readFile func(string) ([]byte, error)) bool {
	for _, name := range disableIPv6Files {
		value, err := readFile(name)
		if errors.Is(err, fs.ErrNotExist) {
			return false
		}
		if err != nil {
			// Unreadable but present: keep IPv6 so a leak stays impossible;
			// a genuinely disabled stack then fails the start visibly.
			continue
		}
		if strings.TrimSpace(string(value)) != "0" {
			return false
		}
	}
	return true
}

// Files listing the host's IPv6 addresses and routes (every table).
const (
	ifInet6File   = "/proc/net/if_inet6"
	ipv6RouteFile = "/proc/net/ipv6_route"
)

func route() (bool, error) { return routeFrom(os.ReadFile) }

// routeFrom reads /proc/net/if_inet6 ("addr ifindex plen scope flags name")
// and /proc/net/ipv6_route ("dst dstlen src srclen nexthop metric refcnt use
// flags name"), and reports whether one interface has a global unicast
// address and a usable default route (::/0, up, not a reject route).
func routeFrom(readFile func(string) ([]byte, error)) (bool, error) {
	addrs, err := readFile(ifInet6File)
	if err != nil {
		return true, err
	}
	global := map[string]bool{}
	for _, line := range strings.Split(string(addrs), "\n") {
		fields := strings.Fields(line)
		if len(fields) < 6 {
			continue
		}
		if a, ok := hexAddr(fields[0]); ok && globalUnicast(a) {
			global[fields[5]] = true
		}
	}
	if len(global) == 0 {
		return false, nil
	}
	routes, err := readFile(ipv6RouteFile)
	if err != nil {
		return true, err
	}
	const rtfUp, rtfReject = 0x1, 0x200
	for _, line := range strings.Split(string(routes), "\n") {
		fields := strings.Fields(line)
		if len(fields) < 10 || fields[1] != "00" || strings.Trim(fields[0], "0") != "" {
			continue
		}
		flags, err := strconv.ParseUint(fields[8], 16, 32)
		if err != nil || flags&rtfUp == 0 || flags&rtfReject != 0 {
			continue
		}
		if global[fields[9]] {
			return true, nil
		}
	}
	return false, nil
}

// hexAddr parses the 32 hex digits /proc uses for an IPv6 address.
func hexAddr(s string) (netip.Addr, bool) {
	raw, err := hex.DecodeString(s)
	if err != nil || len(raw) != 16 {
		return netip.Addr{}, false
	}
	return netip.AddrFrom16([16]byte(raw)), true
}
