//go:build !linux && !windows

package hostipv6

// Darwin keeps IPv6 enabled (it cannot be switched off system-wide), and
// mobile hosts build the tunnel themselves, so the TUN never needs to skip it.
func available() bool { return true }
