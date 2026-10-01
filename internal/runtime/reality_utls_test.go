//go:build with_utls

package runtime

import (
	"os"
	"testing"
	"time"

	"github.com/peakpassvpn/ppvpn-core/profile"
)

// TestFixtureWithRealityStartsInLocalProxyMode starts the shared fixture (a
// VLESS REALITY primary with a Shadowsocks backup) in the unprivileged
// desktop mode. REALITY outbounds only initialize in with_utls builds.
func TestFixtureWithRealityStartsInLocalProxyMode(t *testing.T) {
	data, err := os.ReadFile("../../testdata/profiles/multi-ingress.json")
	if err != nil {
		t.Fatal(err)
	}
	p, err := profile.Parse(data)
	if err != nil {
		t.Fatal(err)
	}
	platform := profile.PlatformCapabilities{Platform: "macos", LocalProxy: profile.LocalProxyCapabilities{Enabled: true, Listen: "127.0.0.1"}, LogLevel: "error"}
	core := newLocalProxyTestCore(t, platform)
	if _, err = core.ApplyProfile(p, time.Now()); err != nil {
		t.Fatal(err)
	}
	if err = core.Start(); err != nil {
		t.Fatal(err)
	}
	defer core.Stop()
	// One local proxy per logical node, plus the routed user (0.5.12).
	if len(core.LocalProxyEndpoints()) != len(p.Nodes) || len(core.LocalProxyMetadata()) != len(p.Nodes)+1 {
		t.Fatal("expected one local proxy per logical node, plus the routed user")
	}
}
