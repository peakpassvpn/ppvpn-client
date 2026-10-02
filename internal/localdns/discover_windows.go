package localdns

import (
	"context"
	"errors"
	"fmt"
	"net/netip"
	"strconv"
	"unsafe"

	"github.com/sagernet/sing/common/control"
	"golang.org/x/sys/windows"
)

var discover discoverFunc = discoverAdapters

// discoverAdapters reads the DNS servers of the adapter that is iface
// (GetAdaptersAddresses, a few milliseconds). Unlike sing-box's local
// transport it does not require a gateway: iface holds the default route.
func discoverAdapters(_ context.Context, iface control.Interface) ([]netip.AddrPort, string, error) {
	const flags = windows.GAA_FLAG_SKIP_UNICAST | windows.GAA_FLAG_SKIP_ANYCAST | windows.GAA_FLAG_SKIP_MULTICAST | windows.GAA_FLAG_SKIP_FRIENDLY_NAME
	size := uint32(15000)
	for {
		buffer := make([]byte, size)
		first := (*windows.IpAdapterAddresses)(unsafe.Pointer(&buffer[0]))
		err := windows.GetAdaptersAddresses(windows.AF_UNSPEC, flags, 0, first, &size)
		if errors.Is(err, windows.ERROR_BUFFER_OVERFLOW) {
			continue
		}
		if err != nil {
			return nil, "adapter", err
		}
		for adapter := first; adapter != nil; adapter = adapter.Next {
			if int(adapter.IfIndex) != iface.Index && int(adapter.Ipv6IfIndex) != iface.Index {
				continue
			}
			var servers []netip.AddrPort
			for server := adapter.FirstDnsServerAddress; server != nil; server = server.Next {
				addr, ok := netip.AddrFromSlice(server.Address.IP())
				if !ok {
					continue
				}
				addr = addr.Unmap()
				if addr.Is6() && addr.IsLinkLocalUnicast() {
					addr = addr.WithZone(strconv.Itoa(int(adapter.Ipv6IfIndex)))
				}
				servers = append(servers, netip.AddrPortFrom(addr, 53))
			}
			return servers, "adapter", nil
		}
		return nil, "adapter", fmt.Errorf("no adapter with index %d", iface.Index)
	}
}
