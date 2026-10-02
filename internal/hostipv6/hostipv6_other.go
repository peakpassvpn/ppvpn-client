//go:build !linux && !windows && !darwin

package hostipv6

// Darwin keeps IPv6 enabled (it cannot be switched off system-wide), and
// mobile hosts build the tunnel themselves, so the TUN never needs to skip it.
func available() bool { return true }

// route keeps IPv6 as before where the core cannot tell.
func route() (bool, error) { return true, nil }
