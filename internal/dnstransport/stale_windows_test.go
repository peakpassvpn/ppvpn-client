package dnstransport

import (
	"fmt"
	"net"
	"os"
	"syscall"
	"testing"
)

// A Windows reset or abort, as net wraps it, is a stale connection.
func TestStaleConnectionWindowsErrnos(t *testing.T) {
	for _, errno := range []syscall.Errno{syscall.WSAECONNRESET, syscall.WSAECONNABORTED} {
		err := fmt.Errorf("read response: %w", &net.OpError{Op: "read", Net: "tcp", Err: os.NewSyscallError("wsarecv", errno)})
		if !staleConnection(err) {
			t.Errorf("%v not stale", err)
		}
	}
}
