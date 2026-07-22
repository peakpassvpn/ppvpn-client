package probe

import (
	"context"
	"github.com/peakpassvpn/ppvpn-core/localproxy"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strconv"
	"testing"
	"time"
)

func TestAvailabilityUsesAuthenticatedNodeProxy(t *testing.T) {
	seenAuth := ""
	proxy := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		seenAuth = r.Header.Get("Proxy-Authorization")
		w.WriteHeader(http.StatusNoContent)
	}))
	defer proxy.Close()
	u, _ := url.Parse(proxy.URL)
	port, _ := strconv.Atoi(u.Port())
	endpoint := localproxy.Endpoint{NodeID: "stable", Listen: u.Hostname(), Port: uint16(port), Username: "user", Password: "password"}
	got := Availability(context.Background(), endpoint, "http://target.invalid/check", time.Second)
	if !got.Success || got.NodeID != "stable" || seenAuth == "" {
		t.Fatalf("%#v auth=%q", got, seenAuth)
	}
}
func TestAvailabilityCancellation(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	got := Availability(ctx, localproxy.Endpoint{NodeID: "n", Listen: "127.0.0.1", Port: 1}, "https://example.invalid", time.Second)
	if got.ErrorCode != "CANCELED" {
		t.Fatalf("%#v", got)
	}
}
