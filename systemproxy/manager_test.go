package systemproxy

import (
	"net"
	"os"
	"path/filepath"
	"strconv"
	"testing"
)

func TestStableEndpointAndSecureState(t *testing.T) {
	dir := t.TempDir()
	if err := os.Chmod(dir, 0o700); err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(dir, "system-proxy.json")
	manager := NewManager(path)
	first, changed, err := manager.Ensure("127.0.0.1", true)
	if err != nil || !changed || first.Port == 0 {
		t.Fatalf("first endpoint: %#v changed=%v err=%v", first, changed, err)
	}
	again, changed, err := manager.Ensure("127.0.0.1", true)
	if err != nil || changed || again != first {
		t.Fatalf("unstable endpoint: %#v -> %#v changed=%v err=%v", first, again, changed, err)
	}
	if info, err := os.Stat(path); err != nil || info.Mode().Perm() != 0o600 {
		t.Fatalf("state permissions: %v %v", info.Mode().Perm(), err)
	}
}

func TestOccupiedPortMigratesAndPersists(t *testing.T) {
	dir := t.TempDir()
	if err := os.Chmod(dir, 0o700); err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(dir, "system-proxy.json")
	manager := NewManager(path)
	first, _, err := manager.Ensure("127.0.0.1", true)
	if err != nil {
		t.Fatal(err)
	}
	listener, err := net.Listen("tcp", net.JoinHostPort(first.Listen, strconv.Itoa(int(first.Port))))
	if err != nil {
		t.Fatal(err)
	}
	defer listener.Close()
	migrated, changed, err := manager.Ensure("127.0.0.1", true)
	if err != nil || !changed || migrated.Port == first.Port {
		t.Fatalf("migration: %#v -> %#v changed=%v err=%v", first, migrated, changed, err)
	}
	restored, changed, err := NewManager(path).Ensure("127.0.0.1", true)
	if err != nil || changed || restored != migrated {
		t.Fatalf("restore: %#v changed=%v err=%v", restored, changed, err)
	}
}

func TestRejectsNonLoopbackListen(t *testing.T) {
	for _, listen := range []string{"", "0.0.0.0", "192.168.1.2", "localhost", "example.com"} {
		if _, _, err := NewManager(filepath.Join(t.TempDir(), "state.json")).Ensure(listen, true); err == nil {
			t.Fatalf("accepted malicious or invalid listen %q", listen)
		}
	}
	for _, listen := range []string{"127.0.0.1", "::1"} {
		if err := ValidateListen(listen); err != nil {
			t.Fatalf("rejected loopback %q: %v", listen, err)
		}
	}
}
