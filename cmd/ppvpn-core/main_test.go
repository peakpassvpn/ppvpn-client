package main

import (
	"os"
	"path/filepath"
	"strings"
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

func TestServeLogsToFileEvenWhenStartupFails(t *testing.T) {
	logPath := filepath.Join(t.TempDir(), "ppvpn-core.log")
	err := run([]string{"serve", "--tun", "--local-proxy=false", "--log-file", logPath})
	if err == nil {
		t.Fatal("serve without socket succeeded")
	}
	data, readErr := os.ReadFile(logPath)
	if readErr != nil {
		t.Fatal(readErr)
	}
	log := string(data)
	for _, want := range []string{`msg="serve starting"`, "tun=true", "local_proxy=false", "level=error", `msg="serve failed"`} {
		if !strings.Contains(log, want) {
			t.Errorf("log missing %q:\n%s", want, log)
		}
	}
}

