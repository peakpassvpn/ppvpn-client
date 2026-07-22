package main

import (
	"os"
	"path/filepath"
	"testing"
)

func TestRotateSessionSecret(t *testing.T) {
	path := filepath.Join(t.TempDir(), "session.secret")
	first, err := rotateSessionSecret(path)
	if err != nil {
		t.Fatal(err)
	}
	second, err := rotateSessionSecret(path)
	if err != nil {
		t.Fatal(err)
	}
	if len(first) < 32 || first == second {
		t.Fatal("session secret was not rotated")
	}
	info, err := os.Stat(path)
	if err != nil || info.Mode().Perm() != 0o600 {
		t.Fatalf("permissions: %v %v", info.Mode().Perm(), err)
	}
}
