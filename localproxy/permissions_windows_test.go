//go:build windows

package localproxy

import (
	"os"
	"path/filepath"
	"testing"

	"golang.org/x/sys/windows"
)

func TestPrivateDescriptorAcceptsAutoInheritedFlag(t *testing.T) {
	for sddl, want := range map[string]bool{
		"D:P(A;;FA;;;SY)(A;;FA;;;BA)":           true,
		"D:PAI(A;;FA;;;SY)(A;;FA;;;BA)":         true,  // what Windows reads back
		"D:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)": true,  // inheritable directory ACEs
		"D:(A;;FA;;;SY)(A;;FA;;;BA)":            false, // not protected
		"D:PAI(A;;FA;;;SY)(A;;FR;;;BU)":         false, // users can read
		"D:PAI(A;;FA;;;SY)(A;;FA;;;WD)":         false, // everyone
		"D:P":                                   false, // empty DACL: nobody, but also unusable
	} {
		descriptor, err := windows.SecurityDescriptorFromString(sddl)
		if err != nil {
			t.Fatal(err)
		}
		if got := privateDescriptor(descriptor); got != want {
			t.Errorf("%s: %v, want %v", sddl, got, want)
		}
	}
}

// TestStateWrittenByCoreIsAcceptedOnReload is the Windows regression test:
// the state file written by save must load again.
func TestStateWrittenByCoreIsAcceptedOnReload(t *testing.T) {
	path := filepath.Join(t.TempDir(), "state", "local-proxies.json")
	first, err := NewManager(path).WithPreferredPort(0).ReconcileForStartup([]string{"node"})
	if err != nil {
		t.Skipf("cannot apply the private ACL in this test environment: %v", err)
	}
	info, err := os.Stat(path)
	if err != nil || !securePermissions(path, info) {
		t.Fatalf("state written by the core is not considered private: %v", err)
	}
	again, err := NewManager(path).WithPreferredPort(0).Ensure([]string{"node"})
	if err != nil || again[0] != first[0] {
		t.Fatalf("reload failed: %#v %v", again, err)
	}
}
