package version

import (
	"encoding/json"
	"strings"
	"testing"

	"github.com/peakpassvpn/ppvpn-core/profile"
)

func TestPublishedCapabilitiesMatchProfileAndHideImplementation(t *testing.T) {
	info := Get()
	if info.CoreAPIVersion != 1 ||
		info.ProfileSchemaVersion != profile.CurrentSchemaVersion ||
		info.FlowAdapterVersion != 1 ||
		info.LocalProxyContractVersion != 1 {
		t.Fatalf("capabilities: %#v", info)
	}
	data, err := json.Marshal(info)
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(string(data), "sing") || strings.Contains(string(data), "stack") {
		t.Fatalf("public version leaked internal runtime: %s", data)
	}
}
