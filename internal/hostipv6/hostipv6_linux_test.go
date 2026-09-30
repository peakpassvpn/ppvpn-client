package hostipv6

import (
	"errors"
	"io/fs"
	"testing"
)

func TestLinuxAvailability(t *testing.T) {
	errDenied := errors.New("permission denied")
	cases := []struct {
		name       string
		all, deflt string
		allErr     error
		want       bool
	}{
		{name: "enabled", all: "0\n", deflt: "0\n", want: true},
		{name: "all disabled", all: "1\n", deflt: "0\n", want: false},
		{name: "default disabled", all: "0\n", deflt: "1\n", want: false},
		{name: "ipv6.disable=1", allErr: fs.ErrNotExist, want: false},
		{name: "unreadable keeps ipv6", allErr: errDenied, deflt: "0\n", want: true},
	}
	for _, tc := range cases {
		got := availableFrom(func(name string) ([]byte, error) {
			switch name {
			case disableIPv6Files[0]:
				return []byte(tc.all), tc.allErr
			case disableIPv6Files[1]:
				if tc.allErr == fs.ErrNotExist {
					return nil, fs.ErrNotExist
				}
				return []byte(tc.deflt), nil
			}
			t.Fatalf("unexpected read %q", name)
			return nil, nil
		})
		if got != tc.want {
			t.Errorf("%s: got %v, want %v", tc.name, got, tc.want)
		}
	}
}
