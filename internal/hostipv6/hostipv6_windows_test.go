package hostipv6

import "testing"

func TestWindowsAvailability(t *testing.T) {
	cases := []struct {
		name     string
		value    uint32
		set      bool
		adapters bool
		want     bool
	}{
		{name: "default", adapters: true, want: true},
		{name: "prefer ipv4 only", value: 0x20, set: true, adapters: true, want: true},
		{name: "tunnel interfaces only", value: 0x01, set: true, adapters: true, want: true},
		{name: "non-tunnel disabled", value: 0x10, set: true, adapters: true, want: false},
		{name: "all disabled", value: 0xFF, set: true, adapters: true, want: false},
		{name: "stack missing", adapters: false, want: false},
	}
	for _, tc := range cases {
		got := availableFrom(func() (uint32, bool) { return tc.value, tc.set }, func() bool { return tc.adapters })
		if got != tc.want {
			t.Errorf("%s: got %v, want %v", tc.name, got, tc.want)
		}
	}
}
