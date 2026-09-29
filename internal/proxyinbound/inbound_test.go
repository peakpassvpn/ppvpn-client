package proxyinbound

import (
	std_bufio "bufio"
	"encoding/base64"
	"strings"
	"testing"
)

func TestVerifierRequiresKnownUserAndExactSecret(t *testing.T) {
	v, err := newVerifier([]User{{Username: "u8f2k-a", Password: "secret"}, {Username: "u8f2k-b-c", Password: "secret"}})
	if err != nil {
		t.Fatal(err)
	}
	for _, tc := range []struct {
		username, password string
		ok                 bool
	}{
		{"u8f2k-a", "secret", true},
		{"u8f2k-b-c", "secret", true},
		{"u8f2k-a", "secreT", false},
		{"u8f2k-a", "secret ", false},
		{"u8f2k-a", "", false},
		{"u8f2k-missing", "secret", false},
		{"", "secret", false},
	} {
		if got := v.verify(tc.username, tc.password); got != tc.ok {
			t.Errorf("verify(%q, %q) = %v", tc.username, tc.password, got)
		}
	}
	for name, users := range map[string][]User{
		"empty":     nil,
		"no secret": {{Username: "a"}},
		"duplicate": {{Username: "a", Password: "x"}, {Username: "a", Password: "y"}},
	} {
		if _, err := newVerifier(users); err == nil {
			t.Errorf("%s accepted", name)
		}
	}
}

func TestBasicProxyAuth(t *testing.T) {
	encoded := base64.StdEncoding.EncodeToString([]byte("u8f2k-node-1:pa:ss"))
	if u, p, ok := basicProxyAuth("Basic " + encoded); !ok || u != "u8f2k-node-1" || p != "pa:ss" {
		t.Fatalf("got %q %q %v", u, p, ok)
	}
	for _, header := range []string{"", "Bearer " + encoded, "Basic !!!", "Basic " + base64.StdEncoding.EncodeToString([]byte("nocolon"))} {
		if _, _, ok := basicProxyAuth(header); ok {
			t.Errorf("accepted %q", header)
		}
	}
}

func TestPeekRequestHeadDoesNotConsume(t *testing.T) {
	raw := "CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\nbody"
	reader := std_bufio.NewReaderSize(strings.NewReader(raw), maxHeaderBytes)
	head, err := peekRequestHead(reader)
	if err != nil || string(head) != strings.TrimSuffix(raw, "body") {
		t.Fatalf("head %q %v", head, err)
	}
	if reader.Buffered() != len(raw) {
		t.Fatalf("request consumed: %d buffered", reader.Buffered())
	}
	huge := "GET / HTTP/1.1\r\nX: " + strings.Repeat("a", maxHeaderBytes) + "\r\n\r\n"
	if _, err = peekRequestHead(std_bufio.NewReaderSize(strings.NewReader(huge), maxHeaderBytes)); err == nil {
		t.Fatal("oversized head accepted")
	}
}
