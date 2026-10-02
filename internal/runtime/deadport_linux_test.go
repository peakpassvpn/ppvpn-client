package runtime

import (
	"syscall"
	"testing"
)

// deadPort returns a loopback port that refuses connections and stays
// reserved for the test: a socket is bound to it but never listens (Linux
// answers a SYN to it with RST). A port from freePort is released at once,
// and Linux hands a just-released port to a later listener now and then (the
// core's own local proxy, another package's server, about once in 2000); a
// "dead" ingress that some listener accepts on looks alive to failover, and a
// dial to it hangs instead of being refused.
func deadPort(t *testing.T) uint16 {
	t.Helper()
	fd, err := syscall.Socket(syscall.AF_INET, syscall.SOCK_STREAM, 0)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = syscall.Close(fd) })
	if err = syscall.Bind(fd, &syscall.SockaddrInet4{Addr: [4]byte{127, 0, 0, 1}}); err != nil {
		t.Fatal(err)
	}
	bound, err := syscall.Getsockname(fd)
	if err != nil {
		t.Fatal(err)
	}
	return uint16(bound.(*syscall.SockaddrInet4).Port)
}
