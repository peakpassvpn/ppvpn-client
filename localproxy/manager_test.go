package localproxy

import (
	"net"
	"os"
	"path/filepath"
	"strconv"
	"testing"
)

func TestStableDistinctEndpointsAndPermissions(t *testing.T) {
	path := filepath.Join(t.TempDir(), "state.json")
	m := NewManager(path)
	first, err := m.Ensure([]string{"b", "a"})
	if err != nil {
		t.Fatal(err)
	}
	if len(first) != 2 || first[0].Port == first[1].Port || first[0].Password == first[1].Password {
		t.Fatal("endpoints not independent")
	}
	again, err := m.Ensure([]string{"a", "b"})
	if err != nil {
		t.Fatal(err)
	}
	if first[0] != again[0] || first[1] != again[1] {
		t.Fatal("endpoints not stable")
	}
	info, err := os.Stat(path)
	if err != nil || info.Mode().Perm() != 0o600 {
		t.Fatalf("permissions: %v %v", info.Mode().Perm(), err)
	}
	if dirInfo, err := os.Stat(filepath.Dir(path)); err != nil || dirInfo.Mode().Perm() != 0o700 {
		t.Fatalf("directory permissions: %v %v", dirInfo.Mode().Perm(), err)
	}
}

func TestStartupReallocatesOnlyOccupiedPort(t *testing.T) {
	path := filepath.Join(t.TempDir(), "state.json")
	manager := NewManager(path)
	first, err := manager.Ensure([]string{"node"})
	if err != nil {
		t.Fatal(err)
	}
	listener, err := net.Listen("tcp", net.JoinHostPort(first[0].Listen, strconv.Itoa(int(first[0].Port))))
	if err != nil {
		t.Fatal(err)
	}
	defer listener.Close()
	reconciled, err := manager.ReconcileForStartup([]string{"node"})
	if err != nil {
		t.Fatal(err)
	}
	if reconciled[0].Port == first[0].Port || reconciled[0].Username != first[0].Username || reconciled[0].Password != first[0].Password {
		t.Fatalf("unexpected reconciliation: %#v -> %#v", first[0], reconciled[0])
	}
}
func TestRejectsWeakStatePermissions(t *testing.T) {
	path := filepath.Join(t.TempDir(), "state.json")
	if err := os.WriteFile(path, []byte(`{"version":1,"endpoints":{}}`), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := NewManager(path).Ensure([]string{"a"}); err == nil {
		t.Fatal("weak permissions accepted")
	}
}

func TestRemovedNodeMappingIsReclaimed(t *testing.T) {
	path := filepath.Join(t.TempDir(), "state.json")
	manager := NewManager(path)
	first, err := manager.Ensure([]string{"keep", "remove"})
	if err != nil {
		t.Fatal(err)
	}
	var removed Endpoint
	for _, endpoint := range first {
		if endpoint.NodeID == "remove" {
			removed = endpoint
		}
	}
	if _, err = manager.Ensure([]string{"keep"}); err != nil {
		t.Fatal(err)
	}
	second, err := manager.Ensure([]string{"keep", "new"})
	if err != nil {
		t.Fatal(err)
	}
	for _, endpoint := range second {
		if endpoint.NodeID == "new" && endpoint.Username == removed.Username {
			t.Fatal("deleted node credentials were reused")
		}
	}
}
