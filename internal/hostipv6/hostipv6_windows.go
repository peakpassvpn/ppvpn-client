package hostipv6

import (
	"errors"
	"unsafe"

	"golang.org/x/sys/windows"
	"golang.org/x/sys/windows/registry"
)

// disabledComponentsNonTunnel is the DisabledComponents bit that disables
// IPv6 on all non-tunnel interfaces, Wintun included (0xFF sets it).
const disabledComponentsNonTunnel = 0x10

func available() bool {
	return availableFrom(disabledComponents, hasIPv6Adapter)
}

func availableFrom(disabled func() (uint32, bool), hasAdapter func() bool) bool {
	if value, ok := disabled(); ok && value&disabledComponentsNonTunnel != 0 {
		return false
	}
	return hasAdapter()
}

// disabledComponents reads the Tcpip6 DisabledComponents policy; ok is false
// when it is not set (the default: IPv6 enabled).
func disabledComponents() (uint32, bool) {
	key, err := registry.OpenKey(registry.LOCAL_MACHINE, `SYSTEM\CurrentControlSet\Services\Tcpip6\Parameters`, registry.QUERY_VALUE)
	if err != nil {
		return 0, false
	}
	defer key.Close()
	value, _, err := key.GetIntegerValue("DisabledComponents")
	if err != nil {
		return 0, false
	}
	return uint32(value), true
}

// hasIPv6Adapter reports whether the IPv6 stack is present at all: with the
// protocol uninstalled or unbound everywhere, AF_INET6 lists no adapter
// (ERROR_NO_DATA). Any other failure keeps IPv6, so a leak stays impossible
// and a genuinely missing stack fails the start visibly.
func hasIPv6Adapter() bool {
	const flags = windows.GAA_FLAG_SKIP_UNICAST | windows.GAA_FLAG_SKIP_ANYCAST | windows.GAA_FLAG_SKIP_MULTICAST | windows.GAA_FLAG_SKIP_DNS_SERVER
	size := uint32(16 * 1024)
	for range 3 {
		buffer := make([]byte, size)
		err := windows.GetAdaptersAddresses(windows.AF_INET6, flags, 0, (*windows.IpAdapterAddresses)(unsafe.Pointer(&buffer[0])), &size)
		switch {
		case err == nil:
			return true
		case errors.Is(err, windows.ERROR_NO_DATA):
			return false
		case !errors.Is(err, windows.ERROR_BUFFER_OVERFLOW):
			return true
		}
	}
	return true
}
