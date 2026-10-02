package runtime

// Runtime tests do not depend on the machine's network: every core starts
// with an IPv6 path (no direct hand-off) unless a test sets hostIPv6Route.
func init() {
	hostIPv6Route = func() (bool, error) { return true, nil }
}
