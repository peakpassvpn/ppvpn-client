package runtime

import (
	"testing"

	"github.com/peakpassvpn/ppvpn-core/internal/config"
	"github.com/peakpassvpn/ppvpn-core/profile"
)

// closeOnSwitch decides by the node the routing kernel recorded, not by
// looking the connection's tags up in the previous build, so it holds even
// if tags stop being stable across builds.
func TestCloseOnSwitchDecidesByRecordedNode(t *testing.T) {
	next := &profile.Profile{Nodes: []profile.Node{{ID: "a"}}}
	// The previous build maps "tag-x" to b (removed) and knows no "tag-new".
	old := &config.BuildResult{OutboundNodes: map[string]string{"tag-x": "b", "tag-a": "a"}}
	decide := closeOnSwitch(old, next, &config.BuildResult{})
	cases := []struct {
		name string
		item trackedView
		want bool
	}{
		{"recorded node removed, tag unknown to the previous build", trackedView{nodeID: "b", outboundTag: "tag-new"}, true},
		{"recorded node kept, tag mapped to a removed node by the previous build", trackedView{nodeID: "a", outboundTag: "tag-x"}, false},
		{"no recorded node: falls back to the previous build's tags", trackedView{outboundTag: "tag-x"}, true},
		{"no recorded node, kept tag", trackedView{outboundTag: "tag-a"}, false},
		{"direct (no node)", trackedView{outboundTag: "direct"}, false},
	}
	for _, tc := range cases {
		if got := decide(nil, tc.item); got != tc.want {
			t.Errorf("%s: close %v, want %v", tc.name, got, tc.want)
		}
	}
}
