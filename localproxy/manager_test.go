package localproxy

import (
	"encoding/json"
	"net"
	"os"
	"path/filepath"
	"runtime"
	"strconv"
	"testing"
	"time"

	"github.com/peakpassvpn/ppvpn-core/internal/privateacl"
)

// holdPort binds 127.0.0.1:port for the test. A concurrent test package may
// probe the same well-known port for an instant, so retry briefly; if it is
// still taken, another process owns it and it is busy either way.
func holdPort(t *testing.T, port uint16) {
	t.Helper()
	for attempt := 0; attempt < 20; attempt++ {
		listener, err := net.Listen("tcp", net.JoinHostPort(Listen, strconv.Itoa(int(port))))
		if err == nil {
			t.Cleanup(func() { listener.Close() })
			return
		}
		time.Sleep(10 * time.Millisecond)
	}
}

func readState(t *testing.T, path string) diskState {
	t.Helper()
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var state diskState
	if err = json.Unmarshal(data, &state); err != nil {
		t.Fatal(err)
	}
	return state
}

func TestSharedEndpointsAreStableAndPrivate(t *testing.T) {
	path := filepath.Join(t.TempDir(), "state.json")
	m := NewManager(path)
	first, err := m.ReconcileForStartup([]string{"b", "a"})
	if err != nil {
		t.Fatal(err)
	}
	if len(first) != 2 || first[0].NodeID != "a" || first[1].NodeID != "b" {
		t.Fatalf("endpoints: %#v", first)
	}
	prefix, nodeID, ok := ParseUsername(first[0].Username)
	if !ok || nodeID != "a" || len(prefix) != 5 {
		t.Fatalf("username %q", first[0].Username)
	}
	for _, e := range first {
		if e.Listen != "127.0.0.1" || e.Port == 0 || e.Port != first[0].Port || e.Password != first[0].Password || len(e.Password) < 40 {
			t.Fatalf("endpoint not shared: %#v", e)
		}
		if e.Username != FormatUsername(prefix, e.NodeID) {
			t.Fatalf("username %q", e.Username)
		}
	}
	// A new manager (restart) and a profile update keep prefix, password and port.
	again, err := NewManager(path).Ensure([]string{"a", "b", "c"})
	if err != nil {
		t.Fatal(err)
	}
	if again[0] != first[0] || again[1] != first[1] || again[2].Username != FormatUsername(prefix, "c") || again[2].Port != first[0].Port {
		t.Fatalf("endpoints not stable: %#v -> %#v", first, again)
	}
	state := readState(t, path)
	if state.Version != StateVersion || state.Prefix != prefix || state.Password != first[0].Password || state.Port != first[0].Port {
		t.Fatalf("persisted state: %#v", state)
	}
	// Private means a 0600 file on Unix and a protected owner/SYSTEM DACL on
	// Windows, where mode bits say nothing; privateacl checks either.
	if exists, err := privateacl.CheckFile(path); err != nil || !exists {
		t.Fatalf("state file not private: %v %v", exists, err)
	}
	if runtime.GOOS != "windows" {
		if dirInfo, err := os.Stat(filepath.Dir(path)); err != nil || dirInfo.Mode().Perm() != 0o700 {
			t.Fatalf("directory permissions: %v %v", dirInfo.Mode().Perm(), err)
		}
	}
}

func TestPrefixesAreRandomLowercaseAlphanumerics(t *testing.T) {
	seen := map[string]bool{}
	for i := 0; i < 32; i++ {
		prefix, err := randomPrefix()
		if err != nil {
			t.Fatal(err)
		}
		if !validPrefix(prefix) {
			t.Fatalf("prefix %q", prefix)
		}
		seen[prefix] = true
	}
	if len(seen) < 30 {
		t.Fatalf("prefixes repeat too often: %d distinct", len(seen))
	}
}

func TestParseUsername(t *testing.T) {
	for _, tc := range []struct {
		username, prefix, nodeID string
		ok                       bool
	}{
		{"u8f2k-hk-001", "u8f2k", "hk-001", true},
		{"u8f2k-3f2c9a1e-0000-4000-8000-000000000001-128", "u8f2k", "3f2c9a1e-0000-4000-8000-000000000001-128", true},
		{"u8f2k--leading", "u8f2k", "-leading", true},
		{"u8f2k-a.b_c", "u8f2k", "a.b_c", true},
		{"u8f2k-", "", "", false},
		{"u8f2k", "u8f2k", "", true}, // the routed user
		{"U8F2K", "", "", false},
		{"u8f2", "", "", false},
		{"U8F2K-node", "", "", false},
		{"u8f2-node", "", "", false},
		{"u8f2kk-node", "", "", false},
		{"-node", "", "", false},
		{"", "", "", false},
	} {
		prefix, nodeID, ok := ParseUsername(tc.username)
		if prefix != tc.prefix || nodeID != tc.nodeID || ok != tc.ok {
			t.Errorf("ParseUsername(%q) = %q, %q, %v", tc.username, prefix, nodeID, ok)
		}
		if tc.ok && FormatUsername(prefix, nodeID) != tc.username {
			t.Errorf("round trip %q", tc.username)
		}
	}
}

func TestStartupPrefers7890AndFallsBackWhenBusy(t *testing.T) {
	holdPort(t, PreferredPort)
	path := filepath.Join(t.TempDir(), "state.json")
	first, err := NewManager(path).ReconcileForStartup([]string{"node"})
	if err != nil {
		t.Fatal(err)
	}
	if first[0].Port == PreferredPort || first[0].Port == 0 {
		t.Fatalf("busy preferred port chosen: %d", first[0].Port)
	}
	if readState(t, path).Port != first[0].Port {
		t.Fatal("fallback port not persisted")
	}
	// The persisted fallback port is tried first next time.
	second, err := NewManager(path).ReconcileForStartup([]string{"node"})
	if err != nil {
		t.Fatal(err)
	}
	if second[0] != first[0] {
		t.Fatalf("persisted port not reused: %#v -> %#v", first[0], second[0])
	}
}

func TestStartupUsesPreferredPortWhenFree(t *testing.T) {
	path := filepath.Join(t.TempDir(), "state.json")
	m := NewManager(path)
	// Use a port known to be free instead of 7890, which may be taken on a
	// developer machine.
	probe, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	m.preferredPort = uint16(probe.Addr().(*net.TCPAddr).Port)
	probe.Close()
	got, err := m.ReconcileForStartup([]string{"node"})
	if err != nil {
		t.Fatal(err)
	}
	if got[0].Port != m.preferredPort {
		t.Fatalf("port %d, want preferred %d", got[0].Port, m.preferredPort)
	}
}

func TestStartupReplacesOccupiedPersistedPortOnly(t *testing.T) {
	path := filepath.Join(t.TempDir(), "state.json")
	manager := NewManager(path)
	first, err := manager.ReconcileForStartup([]string{"node"})
	if err != nil {
		t.Fatal(err)
	}
	listener, err := net.Listen("tcp", net.JoinHostPort(first[0].Listen, strconv.Itoa(int(first[0].Port))))
	if err != nil {
		t.Fatal(err)
	}
	defer listener.Close()
	// A running core (Ensure) keeps its port even though it is "occupied".
	running, err := manager.Ensure([]string{"node"})
	if err != nil || running[0] != first[0] {
		t.Fatalf("running core port changed: %#v %v", running, err)
	}
	reconciled, err := manager.ReconcileForStartup([]string{"node"})
	if err != nil {
		t.Fatal(err)
	}
	if reconciled[0].Port == first[0].Port || reconciled[0].Username != first[0].Username || reconciled[0].Password != first[0].Password {
		t.Fatalf("unexpected reconciliation: %#v -> %#v", first[0], reconciled[0])
	}
}

func TestMigratesVersion1StateInPlace(t *testing.T) {
	path := filepath.Join(t.TempDir(), "state.json")
	legacy := `{"version":1,"endpoints":{"hk-001":{"node_id":"hk-001","listen":"127.0.0.1","port":32145,"username":"old-user","password":"old-password"}}}`
	if err := os.WriteFile(path, []byte(legacy), 0o600); err != nil {
		t.Fatal(err)
	}
	// Every core that wrote version 1 made the file private (on Windows a
	// protected DACL; os.WriteFile alone leaves an inherited one, which is
	// rightly refused).
	if err := privateacl.SecureFile(path); err != nil {
		t.Fatal(err)
	}
	got, err := NewManager(path).Ensure([]string{"hk-001"})
	if err != nil {
		t.Fatal(err)
	}
	if got[0].Username == "old-user" || got[0].Password == "old-password" || got[0].Port == 0 {
		t.Fatalf("legacy credentials survived: %#v", got[0])
	}
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var raw map[string]any
	if err = json.Unmarshal(data, &raw); err != nil {
		t.Fatal(err)
	}
	if raw["version"] != float64(StateVersion) || raw["endpoints"] != nil {
		t.Fatalf("state not upgraded: %s", data)
	}
	state := readState(t, path)
	if !validPrefix(state.Prefix) || state.Password != got[0].Password || state.Port != got[0].Port {
		t.Fatalf("upgraded state: %#v", state)
	}
	if info, err := os.Stat(path); err != nil || info.Mode().Perm() != 0o600 {
		t.Fatalf("permissions after upgrade: %v", err)
	}
	// The upgrade happens once: the next load keeps the generated values.
	again, err := NewManager(path).Ensure([]string{"hk-001"})
	if err != nil || again[0] != got[0] {
		t.Fatalf("upgraded state not stable: %#v %v", again, err)
	}
}

func TestRejectsUnsupportedOrCorruptState(t *testing.T) {
	for name, content := range map[string]string{
		"future":         `{"version":3}`,
		"legacy-no-map":  `{"version":1}`,
		"bad-prefix":     `{"version":2,"prefix":"UPPER","password":"x","port":7890}`,
		"prefix-no-pass": `{"version":2,"prefix":"abcde","password":"","port":7890}`,
		"not-json":       `{`,
	} {
		path := filepath.Join(t.TempDir(), "state.json")
		if err := os.WriteFile(path, []byte(content), 0o600); err != nil {
			t.Fatal(err)
		}
		if _, err := NewManager(path).Ensure([]string{"a"}); err == nil {
			t.Errorf("%s accepted", name)
		}
	}
}

func TestRejectsWeakStatePermissions(t *testing.T) {
	path := filepath.Join(t.TempDir(), "state.json")
	if err := os.WriteFile(path, []byte(`{"version":2,"prefix":"abcde","password":"secret","port":7890}`), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := NewManager(path).Ensure([]string{"a"}); err == nil {
		t.Fatal("weak permissions accepted")
	}
}

func TestRemovedNodeHasNoEndpoint(t *testing.T) {
	path := filepath.Join(t.TempDir(), "state.json")
	manager := NewManager(path)
	if _, err := manager.Ensure([]string{"keep", "remove"}); err != nil {
		t.Fatal(err)
	}
	second, err := manager.Ensure([]string{"keep"})
	if err != nil {
		t.Fatal(err)
	}
	if len(second) != 1 || second[0].NodeID != "keep" {
		t.Fatalf("removed node still served: %#v", second)
	}
}

func TestSystemProxyPortPrefers7891FallsBackAndPersists(t *testing.T) {
	holdPort(t, SystemProxyPreferredPort)
	path := filepath.Join(t.TempDir(), "state.json")
	m := NewManager(path).WithPreferredPort(0)
	local, err := m.ReconcileForStartup([]string{"node"})
	if err != nil {
		t.Fatal(err)
	}
	first, err := m.SystemProxyPort(true, local[0].Port)
	if err != nil {
		t.Fatal(err)
	}
	if first == 0 || first == SystemProxyPreferredPort || first == local[0].Port {
		t.Fatalf("system proxy port %d (local %d)", first, local[0].Port)
	}
	state := readState(t, path)
	if state.SystemProxyPort != first || state.Port != local[0].Port || state.Prefix == "" {
		t.Fatalf("persisted state: %#v", state)
	}
	again, err := NewManager(path).SystemProxyPort(true, local[0].Port)
	if err != nil || again != first {
		t.Fatalf("persisted system proxy port not reused: %d -> %d %v", first, again, err)
	}
	// The local proxy never takes the system proxy port, and vice versa.
	if port, err := NewManager(path).WithSystemProxyPreferredPort(0).SystemProxyPort(true, first); err != nil || port == first {
		t.Fatalf("avoided port reused: %d %v", port, err)
	}
}

func TestSystemProxyPortUsesPreferredWhenFree(t *testing.T) {
	probe, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	preferred := uint16(probe.Addr().(*net.TCPAddr).Port)
	probe.Close()
	port, err := NewManager(filepath.Join(t.TempDir(), "state.json")).WithSystemProxyPreferredPort(preferred).SystemProxyPort(true, 0)
	if err != nil || port != preferred {
		t.Fatalf("port %d, want %d: %v", port, preferred, err)
	}
}

func TestRoutedEndpoint(t *testing.T) {
	if _, ok := RoutedEndpoint(nil); ok {
		t.Fatal("routed endpoint without node endpoints")
	}
	nodes := []Endpoint{
		{NodeID: "a", Listen: Listen, Port: 7890, Username: "u8f2k-a", Password: "secret"},
		{NodeID: "b", Listen: Listen, Port: 7890, Username: "u8f2k-b", Password: "secret"},
	}
	got, ok := RoutedEndpoint(nodes)
	if !ok || got != (Endpoint{Listen: Listen, Port: 7890, Username: "u8f2k", Password: "secret"}) {
		t.Fatalf("routed endpoint %+v, %v", got, ok)
	}
}
