//go:build with_utls

package runtime

import (
	"os"
	"path/filepath"
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
	core := NewWithLocalProxyState(platform, filepath.Join(t.TempDir(), "proxy-state.json"))
	if _, err = core.ApplyProfile(p, time.Now()); err != nil {
		t.Fatal(err)
	}
	if err = core.Start(); err != nil {
		t.Fatal(err)
	}
	defer core.Stop()
	if len(core.LocalProxyMetadata()) != len(p.Nodes) {
		t.Fatal("expected one local proxy per logical node")
	}
}
