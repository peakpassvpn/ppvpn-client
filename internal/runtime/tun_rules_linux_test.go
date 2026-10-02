//go:build linux && !android

package runtime

import (
	"context"
	"fmt"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/peakpassvpn/ppvpn-core/internal/corelog"
	"github.com/peakpassvpn/ppvpn-core/profile"
)

// realTUN runs a core with a real TUN in this network namespace. It changes
// the namespace's routing, so it runs only where that is disposable: as root
// with PPVPN_TEST_REAL_TUN=1, in a network namespace of its own (a container,
// or `ip netns exec` as test/netns/run.sh does in CI), never in the host's.
type realTUN struct {
	core   *Core
	events <-chan Event
	logMu  sync.Mutex
	log    strings.Builder
	probes atomic.Int32
}

// The probe range (TEST-NET-1): blackholed in main, routed direct by the
// profile, so a probe connection shows up in the core's connection log only
// when it went through the TUN, and never leaves the machine either way.
// (Not 198.18.0.0/15: the TUN rejects that fake-ip range before routing.)
const probeNet = "192.0.2.0/24"

func newRealTUN(t *testing.T) *realTUN {
	t.Helper()
	if os.Getenv("PPVPN_TEST_REAL_TUN") != "1" || os.Geteuid() != 0 {
		t.Skip("needs root and PPVPN_TEST_REAL_TUN=1 (privileged container)")
	}
	if !ownNetNamespace() {
		t.Skip("changes the network namespace's rules: runs only in a namespace of its own (container or ip netns)")
	}
	ip(t, "route", "replace", "blackhole", probeNet)
	t.Cleanup(func() { _, _ = exec.Command("ip", "route", "del", "blackhole", probeNet).CombinedOutput() })

	f := &realTUN{}
	platform := profile.PlatformCapabilities{Platform: "linux", TUN: profile.TUNCapabilities{Enabled: true}, LogLevel: "error"}
	f.core = NewWithLocalProxyState(platform, filepath.Join(t.TempDir(), "proxy-state.json"))
	logger := corelog.New(writerFunc(func(p []byte) (int, error) {
		f.logMu.Lock()
		defer f.logMu.Unlock()
		return f.log.Write(p)
	}))
	_ = logger.SetLevel(corelog.LevelDebug)
	f.core.SetLogger(logger)
	ctx, cancel := context.WithCancel(context.Background())
	t.Cleanup(cancel)
	f.events = f.core.Subscribe(ctx, 64)
	p := &profile.Profile{
		SchemaVersion: profile.CurrentSchemaVersion, Revision: "r1", ExpiresAt: time.Now().Add(time.Hour),
		Nodes: []profile.Node{{ID: "a", EntryKey: "cn-optimized", Capabilities: profile.Capabilities{TCP: true}, Ingresses: []profile.Ingress{
			localSSIngress(profile.IngressRolePrimary, "a0", 0, startShadowsocksServer(t)),
		}}},
		Selection: profile.Selection{Mode: "manual", DefaultNodeID: "a"},
		Routing: profile.Routing{
			Rules: []profile.RoutingRule{{ID: "probe", Match: profile.RoutingMatch{IPCIDRs: []string{probeNet}}, Action: profile.RoutingAction{Type: "direct"}}},
			Final: profile.RoutingAction{Type: "proxy", Target: "selected"},
		},
	}
	if _, err := f.core.ApplyProfile(p, time.Now()); err != nil {
		t.Fatal(err)
	}
	if err := f.core.Start(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = f.core.Stop() })
	t.Cleanup(func() {
		if t.Failed() {
			t.Logf("guard log:\n%s", grepLines(f.logged(), "tun routing"))
		}
	})
	if got := f.core.Status().TunRouting; got != "ok" {
		t.Fatalf("tun_routing %q after start, want ok", got)
	}
	return f
}

// ownNetNamespace reports whether this process runs in a network namespace
// that is not the host's: inside a container (/.dockerenv; its PID 1 shares
// the container's namespace), or in a namespace other than PID 1's, as with
// `ip netns exec` on a host. Unknown counts as not.
func ownNetNamespace() bool {
	if _, err := os.Stat("/.dockerenv"); err == nil {
		return true
	}
	self, err := os.Readlink("/proc/self/ns/net")
	if err != nil {
		return false
	}
	init, err := os.Readlink("/proc/1/ns/net")
	return err == nil && self != init
}

func (f *realTUN) logged() string {
	f.logMu.Lock()
	defer f.logMu.Unlock()
	return f.log.String()
}

// routed dials a new probe address and reports whether the core routed it,
// i.e. whether it went through the TUN.
func (f *realTUN) routed(t *testing.T) bool {
	t.Helper()
	n := f.probes.Add(1)
	destination := fmt.Sprintf("192.0.2.%d:%d", n%250+1, 9000+n)
	conn, err := net.DialTimeout("tcp", destination, 500*time.Millisecond)
	if err == nil {
		_ = conn.Close()
	}
	deadline := time.Now().Add(time.Second)
	for {
		if strings.Contains(f.logged(), "destination="+destination) {
			return true
		}
		if time.Now().After(deadline) {
			return false
		}
		time.Sleep(20 * time.Millisecond)
	}
}

func ip(t *testing.T, args ...string) string {
	t.Helper()
	out, err := exec.Command("ip", args...).CombinedOutput()
	if err != nil {
		t.Fatalf("ip %s: %v\n%s", strings.Join(args, " "), err, out)
	}
	return string(out)
}

var rulePriority = regexp.MustCompile(`^(\d+):`)

// tunRules is the TUN's rules (priorities 9091..9101, both families) and
// the routes of its table, as `ip` prints them, sorted.
func tunRules(t *testing.T) []string {
	t.Helper()
	var out []string
	for _, family := range []string{"-4", "-6"} {
		for line := range strings.SplitSeq(ip(t, family, "rule", "show"), "\n") {
			match := rulePriority.FindStringSubmatch(line)
			if match == nil {
				continue
			}
			if priority, _ := strconv.Atoi(match[1]); priority >= 9091 && priority <= 9101 {
				out = append(out, family+" "+strings.TrimSpace(line))
			}
		}
		for line := range strings.SplitSeq(ip(t, family, "route", "show", "table", "2091"), "\n") {
			if line = strings.TrimSpace(line); line != "" {
				out = append(out, family+" route "+line)
			}
		}
	}
	slices.Sort(out)
	return out
}

func deleteRules(t *testing.T, priorities ...int) {
	t.Helper()
	for _, family := range []string{"-4", "-6"} {
		for _, priority := range priorities {
			for exec.Command("ip", family, "rule", "del", "priority", strconv.Itoa(priority)).Run() == nil {
			}
		}
	}
}

func waitRules(t *testing.T, want []string, within time.Duration) {
	t.Helper()
	deadline := time.Now().Add(within)
	for {
		got := tunRules(t)
		if slices.Equal(got, want) {
			return
		}
		if time.Now().After(deadline) {
			t.Fatalf("TUN rules not back within %s:\ngot\n%s\nwant\n%s", within, strings.Join(got, "\n"), strings.Join(want, "\n"))
		}
		time.Sleep(20 * time.Millisecond)
	}
}

// TestTUNRulesRestoredAfterDeletion deletes the TUN's policy routing the
// ways seen in the field (everything, as networkd does on a link down; just
// the goto target; the table's routes) and requires each to be put back,
// identical, within a second, with traffic in the TUN again. With
// PPVPN_TEST_TUN_RULES_NO_RESTORE=1 (restoring off) it must fail: that run
// proves the test sees the bypass.
func TestTUNRulesRestoredAfterDeletion(t *testing.T) {
	f := newRealTUN(t)
	want := tunRules(t)
	if len(want) == 0 {
		t.Fatal("sing-tun installed no rules")
	}
	if !f.routed(t) {
		t.Fatalf("probe not routed through the TUN at start:\n%s", f.logged())
	}
	for _, tc := range []struct {
		name   string
		delete func()
	}{
		{"all rules", func() { deleteRules(t, 9091, 9092, 9093, 9094, 9095, 9096, 9097, 9098, 9099, 9100, 9101) }},
		{"goto target", func() { deleteRules(t, 9101) }},
		{"table routes", func() {
			_, _ = exec.Command("ip", "-4", "route", "flush", "table", "2091").CombinedOutput()
			_, _ = exec.Command("ip", "-6", "route", "flush", "table", "2091").CombinedOutput()
		}},
	} {
		t.Logf("deleting %s", tc.name)
		tc.delete()
		waitRules(t, want, time.Second)
		if !f.routed(t) {
			t.Fatalf("%s: probe bypassed the TUN after the restore:\n%s", tc.name, f.logged())
		}
		if got := f.core.Status().TunRouting; got != "ok" {
			t.Fatalf("%s: tun_routing %q", tc.name, got)
		}
	}
	if !strings.Contains(f.logged(), `msg="tun routing rules restored"`) {
		t.Fatalf("no restore logged:\n%s", f.logged())
	}
}

// TestTUNRulesBrokenIsReported: rules that stay missing set the status to
// broken and send TunRoutingBroken. Root can always add them back, so a
// failing restore cannot be staged; the test runs with restoring off, where
// the guard reports what it would otherwise fix.
func TestTUNRulesBrokenIsReported(t *testing.T) {
	if os.Getenv("PPVPN_TEST_TUN_RULES_NO_RESTORE") != "1" {
		t.Skip("runs with restoring off (PPVPN_TEST_TUN_RULES_NO_RESTORE=1)")
	}
	f := newRealTUN(t)
	deleteRules(t, 9101)
	deadline := time.After(2 * time.Second)
	for {
		select {
		case event := <-f.events:
			if event.Type != EventTunRoutingBroken {
				continue
			}
			if len(event.Missing) == 0 || event.Error == "" {
				t.Fatalf("event: %+v", event)
			}
			if got := f.core.Status().TunRouting; got != "broken" {
				t.Fatalf("tun_routing %q, want broken", got)
			}
			return
		case <-deadline:
			t.Fatalf("no TunRoutingBroken:\n%s", f.logged())
		}
	}
}

// TestTUNRulesSurviveNetworkdLinkFlap reproduces the field report: with
// systemd-networkd managing a link (ManageForeignRoutingPolicyRules on, its
// default), taking the link down and up makes networkd drop the TUN's
// rules. Run in a container whose PID 1 is systemd, with networkd managing
// PPVPN_TEST_FLAP_LINK. With restoring off it must fail, which shows
// networkd did delete them.
func TestTUNRulesSurviveNetworkdLinkFlap(t *testing.T) {
	link := os.Getenv("PPVPN_TEST_FLAP_LINK")
	if link == "" {
		t.Skip("needs PPVPN_TEST_FLAP_LINK: a link managed by systemd-networkd")
	}
	f := newRealTUN(t)
	want := tunRules(t)
	if command := os.Getenv("PPVPN_TEST_FLAP_COMMAND"); command != "" {
		// Another trigger to try (networkctl reconfigure, a networkd restart).
		if out, err := exec.Command("sh", "-c", command).CombinedOutput(); err != nil {
			t.Fatalf("%s: %v\n%s", command, err, out)
		}
	} else {
		ip(t, "link", "set", link, "down")
		time.Sleep(5 * time.Second)
		ip(t, "link", "set", link, "up")
	}
	// networkd reconfigures the link once it is up; give it time to drop
	// the rules, then expect them back.
	time.Sleep(3 * time.Second)
	waitRules(t, want, time.Second)
	if !f.routed(t) {
		t.Fatalf("probe bypassed the TUN after the link flap:\n%s", f.logged())
	}
	if !strings.Contains(f.logged(), `msg="tun routing rules restored"`) {
		t.Fatalf("networkd deleted nothing (no restore logged): the flap did not reproduce the report\n%s", f.logged())
	}
	t.Logf("restores:\n%s", grepLines(f.logged(), "tun routing rules restored"))
}

func grepLines(text, needle string) string {
	var out []string
	for line := range strings.SplitSeq(text, "\n") {
		if strings.Contains(line, needle) {
			out = append(out, line)
		}
	}
	return strings.Join(out, "\n")
}
