//go:build windows

package privateacl

import (
	"bytes"
	"errors"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"

	"github.com/peakpassvpn/ppvpn-core/internal/corelog"
	"golang.org/x/sys/windows"
)

const testUser = "S-1-5-21-1111111111-2222222222-3333333333-1001"

func sid(t *testing.T, s string) *windows.SID {
	t.Helper()
	value, err := windows.StringToSid(s)
	if err != nil {
		t.Fatal(err)
	}
	return value
}

func TestTrusteesDependOnProcessUser(t *testing.T) {
	user, err := trustees(sid(t, testUser))
	if err != nil {
		t.Fatal(err)
	}
	if len(user) != 2 || user[0].String() != testUser || !user[1].IsWellKnown(windows.WinLocalSystemSid) {
		t.Fatalf("normal user: %v", user)
	}
	system, err := trustees(sid(t, "S-1-5-18"))
	if err != nil {
		t.Fatal(err)
	}
	if len(system) != 2 || !system[0].IsWellKnown(windows.WinLocalSystemSid) || !system[1].IsWellKnown(windows.WinBuiltinAdministratorsSid) {
		t.Fatalf("LocalSystem: %v", system)
	}
}

func TestPrivateForTable(t *testing.T) {
	user, err := trustees(sid(t, testUser))
	if err != nil {
		t.Fatal(err)
	}
	system, err := trustees(sid(t, "S-1-5-18"))
	if err != nil {
		t.Fatal(err)
	}
	for _, tc := range []struct {
		sddl         string
		user, system bool
	}{
		{"D:P(A;;FA;;;" + testUser + ")(A;;FA;;;SY)", true, false},
		{"D:PAI(A;OICI;FA;;;" + testUser + ")(A;OICI;FA;;;SY)", true, false},
		{"D:PAI(A;;FA;;;" + testUser + ")", true, false},
		{"D:P(A;;FA;;;SY)(A;;FA;;;BA)", false, true}, // 0.4.0 / service ACL
		{"D:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)", false, true},
		{"D:PAI(D;;FA;;;WD)(A;;FA;;;" + testUser + ")", true, false}, // deny ignored
		{"D:(A;;FA;;;" + testUser + ")(A;;FA;;;SY)", false, false},   // not protected
		{"D:PAI(A;;FA;;;" + testUser + ")(A;;FR;;;BU)", false, false},
		{"D:PAI(A;;FA;;;" + testUser + ")(A;;FA;;;S-1-5-21-1-2-3-1002)", false, false}, // another user
		{"D:PAI(A;;FA;;;SY)(A;;FA;;;WD)", false, false},
		{"D:P", false, false},
	} {
		descriptor, err := windows.SecurityDescriptorFromString(tc.sddl)
		if err != nil {
			t.Fatal(err)
		}
		if got := privateFor(descriptor, user); got != tc.user {
			t.Errorf("user %s: %v", tc.sddl, got)
		}
		if got := privateFor(descriptor, system); got != tc.system {
			t.Errorf("system %s: %v", tc.sddl, got)
		}
	}
}

// TestRoundTripAsCurrentUser writes and reads back private state as the
// real process user, which is what the standard core does.
func TestRoundTripAsCurrentUser(t *testing.T) {
	dir := filepath.Join(t.TempDir(), "state")
	if err := os.MkdirAll(dir, 0o700); err != nil {
		t.Fatal(err)
	}
	if err := SecureDirectory(dir); err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(dir, "state.json")
	tmp, err := os.CreateTemp(dir, ".tmp-*")
	if err != nil {
		t.Fatalf("create in private directory: %v", err)
	}
	if err = SecureFile(tmp.Name()); err != nil {
		t.Fatal(err)
	}
	if _, err = tmp.WriteString("secret"); err != nil {
		t.Fatal(err)
	}
	tmp.Close()
	if err = os.Rename(tmp.Name(), path); err != nil {
		t.Fatal(err)
	}
	exists, err := CheckFile(path)
	if err != nil || !exists {
		t.Fatalf("check: %v %v", exists, err)
	}
	data, err := os.ReadFile(path)
	if err != nil || string(data) != "secret" {
		t.Fatalf("read back: %q %v", data, err)
	}
	if exists, err = CheckFile(filepath.Join(dir, "missing.json")); exists || err != nil {
		t.Fatalf("missing: %v %v", exists, err)
	}
}

// TestRepairsLegacyACLAsOwner reproduces a 0.4.0 state directory and file
// (SYSTEM + Administrators only) created by this user, which owns them and
// can therefore rewrite their DACL.
func TestRepairsLegacyACLAsOwner(t *testing.T) {
	user, err := currentUser()
	if err != nil {
		t.Fatal(err)
	}
	if user.IsWellKnown(windows.WinLocalSystemSid) {
		t.Skip("legacy repair applies to normal users")
	}
	dir := filepath.Join(t.TempDir(), "state")
	if err = os.MkdirAll(dir, 0o700); err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(dir, "state.json")
	if err = os.WriteFile(path, []byte("secret"), 0o600); err != nil {
		t.Fatal(err)
	}
	legacy, err := windows.SecurityDescriptorFromString("D:P(A;;FA;;;SY)(A;;FA;;;BA)")
	if err != nil {
		t.Fatal(err)
	}
	dacl, _, err := legacy.DACL()
	if err != nil {
		t.Fatal(err)
	}
	for _, p := range []string{path, dir} {
		if err = windows.SetNamedSecurityInfo(p, windows.SE_FILE_OBJECT, windows.DACL_SECURITY_INFORMATION|windows.PROTECTED_DACL_SECURITY_INFORMATION, nil, nil, dacl, nil); err != nil {
			t.Fatal(err)
		}
	}
	var log bytes.Buffer
	SetLogger(corelog.New(&log))
	t.Cleanup(func() { SetLogger(nil) })
	if err = SecureDirectory(dir); err != nil {
		t.Fatalf("repair directory as owner: %v", err)
	}
	if exists, err := CheckFile(path); err != nil || !exists {
		t.Fatalf("repair file as owner: %v %v", exists, err)
	}
	if data, err := os.ReadFile(path); err != nil || string(data) != "secret" {
		t.Fatalf("read after repair: %q %v", data, err)
	}
	// One info line per repaired object with path and old/new ACL, and no
	// file contents.
	lines := strings.Split(strings.TrimSpace(log.String()), "\n")
	if len(lines) != 2 || strings.Contains(log.String(), "secret") {
		t.Fatalf("repair log: %s", log.String())
	}
	for i, p := range []string{dir, path} {
		line := lines[i]
		if !strings.Contains(line, "path="+p+" ") && !strings.Contains(line, "path="+strconv.Quote(p)+" ") {
			t.Errorf("line %d missing path %s: %s", i, p, line)
		}
		for _, want := range []string{"level=info", "repaired ppvpn-core 0.4.0 private ACL", "old_acl=", "(A;;FA;;;BA)", "new_acl=", user.String()} {
			if !strings.Contains(line, want) {
				t.Errorf("line %d missing %q: %s", i, want, line)
			}
		}
	}
	// An already private object is not logged again.
	log.Reset()
	if err = SecureDirectory(dir); err != nil {
		t.Fatal(err)
	}
	if _, err = CheckFile(path); err != nil || log.Len() != 0 {
		t.Fatalf("second pass: %v %q", err, log.String())
	}
}

func TestBroaderACLIsNotRepaired(t *testing.T) {
	path := filepath.Join(t.TempDir(), "state.json")
	if err := os.WriteFile(path, []byte("secret"), 0o600); err != nil {
		t.Fatal(err)
	}
	// A fresh temp file inherits the (non-protected) temp directory ACL.
	if _, err := CheckFile(path); !errors.Is(err, ErrNotPrivate) {
		t.Fatalf("inherited ACL accepted or repaired: %v", err)
	}
}

func TestAccessErrorNamesPathAndRemedy(t *testing.T) {
	err := accessError(`C:\Users\u\AppData\Local\PPVPN\core\state`, windows.ERROR_ACCESS_DENIED)
	var access *AccessError
	if !errors.As(err, &access) || !errors.Is(err, windows.ERROR_ACCESS_DENIED) {
		t.Fatalf("%T %v", err, err)
	}
	message := err.Error()
	for _, want := range []string{`C:\Users\u\AppData\Local\PPVPN\core\state`, "icacls", "Nothing was deleted"} {
		if !contains(message, want) {
			t.Errorf("missing %q: %s", want, message)
		}
	}
}

func contains(s, sub string) bool { return strings.Contains(s, sub) }
