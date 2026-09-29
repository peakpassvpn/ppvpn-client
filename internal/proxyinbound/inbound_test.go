package proxyinbound

import (
	std_bufio "bufio"
	"context"
	"encoding/base64"
	"errors"
	"io"
	"net"
	"net/http"
	"strings"
	"testing"
	"time"

	"github.com/sagernet/sing-box/adapter"
	"github.com/sagernet/sing-box/log"
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

// serveOne runs the inbound's connection handler for every accepted
// connection on a loopback listener. Only rejected requests are sent, so the
// router is never reached.
func serveOne(t *testing.T, users []User) string {
	t.Helper()
	in, err := New(context.Background(), nil, log.NewNOPFactory().Logger(), "test", Options{Users: users})
	if err != nil {
		t.Fatal(err)
	}
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { ln.Close() })
	go func() {
		for {
			conn, err := ln.Accept()
			if err != nil {
				return
			}
			go in.(*Inbound).NewConnectionEx(context.Background(), conn, adapter.InboundContext{}, nil)
		}
	}()
	return ln.Addr().String()
}

// TestHTTPAuthFailureReturns407ThenClosesGracefully covers CONNECT and plain
// requests without auth, with a wrong password and with an unknown user: each
// reads back a complete 407 challenge followed by a clean EOF, never a reset,
// even when the client already sent bytes past the request head.
func TestHTTPAuthFailureReturns407ThenClosesGracefully(t *testing.T) {
	const user, secret = "u8f2k-node-1", "secret"
	addr := serveOne(t, []User{{Username: user, Password: secret}})
	basic := func(u, p string) string {
		return "Proxy-Authorization: Basic " + base64.StdEncoding.EncodeToString([]byte(u+":"+p)) + "\r\n"
	}
	cases := map[string]string{
		"CONNECT no auth":        "CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n",
		"CONNECT wrong password": "CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n" + basic(user, "wrong") + "\r\n",
		"CONNECT unknown user":   "CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n" + basic("u8f2k-missing", secret) + "\r\n",
		"CONNECT not basic":      "CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\nProxy-Authorization: Bearer x\r\n\r\n",
		// An optimistic client sends its TLS ClientHello right after CONNECT.
		"CONNECT pipelined":   "CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n\x16\x03\x01\x02\x00" + strings.Repeat("\x01", 512),
		"GET no auth":         "GET http://example.com/ HTTP/1.1\r\nHost: example.com\r\n\r\n",
		"POST wrong password": "POST http://example.com/ HTTP/1.1\r\nHost: example.com\r\nContent-Length: 4096\r\n" + basic(user, "wrong") + "\r\n" + strings.Repeat("b", 4096),
	}
	for name, request := range cases {
		t.Run(name, func(t *testing.T) {
			conn, err := net.DialTimeout("tcp", addr, time.Second)
			if err != nil {
				t.Fatal(err)
			}
			defer conn.Close()
			_ = conn.SetDeadline(time.Now().Add(5 * time.Second))
			if _, err = io.WriteString(conn, request); err != nil {
				t.Fatal(err)
			}
			reader := std_bufio.NewReader(conn)
			response, err := http.ReadResponse(reader, nil)
			if err != nil {
				t.Fatalf("read response: %v", err)
			}
			if response.StatusCode != http.StatusProxyAuthRequired {
				t.Fatalf("status %q", response.Status)
			}
			if got := response.Header.Get("Proxy-Authenticate"); !strings.HasPrefix(got, `Basic realm="ppvpn"`) {
				t.Fatalf("Proxy-Authenticate %q", got)
			}
			if response.ContentLength != 0 || !response.Close {
				t.Fatalf("Content-Length %d, close %v", response.ContentLength, response.Close)
			}
			// The server half-closes after the challenge: EOF, not RST.
			if n, err := reader.Read(make([]byte, 1)); n != 0 || !errors.Is(err, io.EOF) {
				t.Fatalf("after 407: %d %v", n, err)
			}
		})
	}
}

// TestSOCKS5AuthFailureRepliesThenClosesGracefully covers a wrong password and
// an unknown user: the client reads the complete RFC 1929 failure reply and
// then a clean EOF, never a reset, even with a CONNECT request pipelined
// behind the credentials.
func TestSOCKS5AuthFailureRepliesThenClosesGracefully(t *testing.T) {
	const user, secret = "u8f2k-node-1", "secret"
	addr := serveOne(t, []User{{Username: user, Password: secret}})
	credentials := func(u, p string) []byte {
		out := append([]byte{1, byte(len(u))}, u...)
		return append(append(out, byte(len(p))), p...)
	}
	connect := []byte{5, 1, 0, 1, 127, 0, 0, 1, 0, 80}
	for name, request := range map[string][]byte{
		"wrong password":           credentials(user, "wrong"),
		"unknown user":             credentials("u8f2k-missing", secret),
		"wrong password pipelined": append(credentials(user, "wrong"), connect...),
	} {
		t.Run(name, func(t *testing.T) {
			conn, err := net.DialTimeout("tcp", addr, time.Second)
			if err != nil {
				t.Fatal(err)
			}
			defer conn.Close()
			_ = conn.SetDeadline(time.Now().Add(5 * time.Second))
			if _, err = conn.Write([]byte{5, 1, 2}); err != nil {
				t.Fatal(err)
			}
			reply := make([]byte, 2)
			if _, err = io.ReadFull(conn, reply); err != nil || reply[0] != 5 || reply[1] != 2 {
				t.Fatalf("method reply %v %v", reply, err)
			}
			if _, err = conn.Write(request); err != nil {
				t.Fatal(err)
			}
			if _, err = io.ReadFull(conn, reply); err != nil {
				t.Fatalf("read auth reply: %v", err)
			}
			if reply[0] != 1 || reply[1] == 0 {
				t.Fatalf("auth reply %v", reply)
			}
			if n, err := conn.Read(make([]byte, 1)); n != 0 || !errors.Is(err, io.EOF) {
				t.Fatalf("after failure reply: %d %v", n, err)
			}
		})
	}
}
