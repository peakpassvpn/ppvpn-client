//go:build !windows

package privateacl

import (
	"errors"
	"os"
	"path/filepath"
	"testing"
)

func TestUnixModes(t *testing.T) {
	dir := filepath.Join(t.TempDir(), "state")
	if err := os.Mkdir(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := SecureDirectory(dir); err != nil {
		t.Fatal(err)
	}
	if info, _ := os.Stat(dir); info.Mode().Perm() != 0o700 {
		t.Fatalf("dir mode %v", info.Mode().Perm())
	}
	path := filepath.Join(dir, "f")
	if exists, err := CheckFile(path); exists || err != nil {
		t.Fatalf("missing: %v %v", exists, err)
	}
	if err := os.WriteFile(path, nil, 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := CheckFile(path); !errors.Is(err, ErrNotPrivate) {
		t.Fatalf("0644 accepted: %v", err)
	}
	if err := SecureFile(path); err != nil {
		t.Fatal(err)
	}
	if exists, err := CheckFile(path); !exists || err != nil {
		t.Fatalf("0600: %v %v", exists, err)
	}
}
