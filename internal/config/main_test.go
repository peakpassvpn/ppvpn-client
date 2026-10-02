package config

import (
	"os"
	"testing"
)

// TestMain renders sing-box's local transport for dns-local whatever the OS
// the tests run on, so goldens are the same on macOS and Linux; tests of the
// core's own transport set ownLocalDNS themselves.
func TestMain(m *testing.M) {
	ownLocalDNS = func() bool { return false }
	os.Exit(m.Run())
}
