//go:build !linux

package runtime

import "testing"

// deadPort is a released port outside Linux: macOS silently drops a SYN to a
// bound but not listening socket (the dial times out instead of being
// refused), and it was not seen handing a just-released port to the next
// listener (0 in 20000 tries, against about 1 in 2000 on Linux). The runtime
// integration tests do not run on Windows.
func deadPort(t *testing.T) uint16 {
	t.Helper()
	return freePort(t)
}
