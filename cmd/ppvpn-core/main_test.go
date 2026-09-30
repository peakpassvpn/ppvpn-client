package main

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	singtun "github.com/sagernet/sing-tun"
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

func TestServeRejectsTUNStackMissingFromBuild(t *testing.T) {
	if singtun.WithGVisor {
		t.Skip("this build includes gVisor")
	}
	dir := t.TempDir()
	logPath := filepath.Join(dir, "ppvpn-core.log")
	err := run([]string{"serve", "--socket", filepath.Join(dir, "core.sock"), "--session-secret-file", filepath.Join(dir, "session.secret"),
		"--state-dir", filepath.Join(dir, "state"), "--tun", "--tun-stack=mixed", "--local-proxy=false", "--log-file", logPath})
	if err == nil || !strings.Contains(err.Error(), "with_gvisor") {
		t.Fatalf("err = %v", err)
	}
	if data, _ := os.ReadFile(logPath); !strings.Contains(string(data), "with_gvisor") {
		t.Fatalf("log: %s", data)
	}
}

func TestServeValidatesLocalDNSServers(t *testing.T) {
	dir := t.TempDir()
	logPath := filepath.Join(dir, "ppvpn-core.log")
	if err := run([]string{"serve", "--tun", "--local-dns-servers", "10.10.0.3,not-an-ip", "--log-file", logPath}); err == nil || !strings.Contains(err.Error(), `invalid local DNS server "not-an-ip"`) {
		t.Fatalf("invalid entry: %v", err)
	}
	if err := run([]string{"serve", "--local-dns-servers", "10.10.0.3", "--log-file", logPath}); err == nil || !strings.Contains(err.Error(), "requires --tun") {
		t.Fatalf("without --tun: %v", err)
	}
	// Valid entries are logged with the one dns-local will use; startup then
	// fails later for the missing socket.
	_ = run([]string{"serve", "--tun", "--local-proxy=false", "--local-dns-servers", " 172.19.0.2 , 10.10.0.3,[fe80::1%en0]:5353", "--log-file", logPath})
	data, _ := os.ReadFile(logPath)
	if !strings.Contains(string(data), `msg="local dns servers" given=172.19.0.2,10.10.0.3,[fe80::1%en0]:5353 selected=10.10.0.3:53`) {
		t.Fatalf("log:\n%s", data)
	}
}
