package redact

import (
	"strings"
	"testing"
)

func TestJSON(t *testing.T) {
	got := string(JSON([]byte(`{"password":"pw","nested":{"uuid":"id"},"name":"ok"}`)))
	if strings.Contains(got, `:"pw"`) || strings.Contains(got, `:"id"`) || !strings.Contains(got, `:"ok"`) {
		t.Fatal(got)
	}
}
func TestProxyURL(t *testing.T) {
	got := Text("socks5://user:password@127.0.0.1:1")
	if strings.Contains(got, "password") {
		t.Fatal(got)
	}
}
