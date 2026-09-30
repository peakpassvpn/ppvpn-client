package runtime

import (
	"bufio"
	"encoding/base64"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/peakpassvpn/ppvpn-core/localproxy"
	"github.com/peakpassvpn/ppvpn-core/profile"
)

// newLocalProxyTestCore is NewWithLocalProxyState without the 7890/7891
// preferences, so parallel test packages and a developer's running proxy never
// compete for the same port.
func newLocalProxyTestCore(t *testing.T, platform profile.PlatformCapabilities) *Core {
	t.Helper()
	core := NewWithLocalProxyState(platform, filepath.Join(t.TempDir(), "proxy-state.json"))
	core.proxyManager.WithPreferredPort(0).WithSystemProxyPreferredPort(0)
	return core
}

func proxyAddress(endpoint localproxy.Endpoint) string {
	return net.JoinHostPort(endpoint.Listen, strconv.Itoa(int(endpoint.Port)))
}

// httpConnect opens a CONNECT tunnel and returns the status line.
func httpConnect(endpoint localproxy.Endpoint, username, password, target string) (net.Conn, string, error) {
	conn, err := net.DialTimeout("tcp", proxyAddress(endpoint), time.Second)
	if err != nil {
		return nil, "", err
	}
	_ = conn.SetDeadline(time.Now().Add(5 * time.Second))
	authorization := base64.StdEncoding.EncodeToString([]byte(username + ":" + password))
	request := "CONNECT " + target + " HTTP/1.1\r\nHost: " + target + "\r\nProxy-Authorization: Basic " + authorization + "\r\n\r\n"
	if _, err = io.WriteString(conn, request); err != nil {
		conn.Close()
		return nil, "", err
	}
	reader := bufio.NewReader(conn)
	response, err := http.ReadResponse(reader, nil)
	if err != nil {
		conn.Close()
		return nil, "", err
	}
	status := response.Status
	if response.StatusCode == http.StatusProxyAuthRequired && !strings.HasPrefix(response.Header.Get("Proxy-Authenticate"), "Basic ") {
		conn.Close()
		return nil, "", fmt.Errorf("407 without Basic challenge")
	}
	return conn, status, nil
}

// socks5Connect authenticates and issues CONNECT; it returns the RFC 1929
// status (0 = success) and, on success, the open tunnel.
func socks5Connect(endpoint localproxy.Endpoint, username, password, target string) (net.Conn, byte, error) {
	conn, err := net.DialTimeout("tcp", proxyAddress(endpoint), time.Second)
	if err != nil {
		return nil, 0, err
	}
	_ = conn.SetDeadline(time.Now().Add(5 * time.Second))
	fail := func(err error) (net.Conn, byte, error) { conn.Close(); return nil, 0, err }
	if _, err = conn.Write([]byte{5, 1, 2}); err != nil {
		return fail(err)
	}
	reply := make([]byte, 2)
	if _, err = io.ReadFull(conn, reply); err != nil || reply[1] != 2 {
		return fail(fmt.Errorf("method reply %v: %v", reply, err))
	}
	request := append([]byte{1, byte(len(username))}, username...)
	request = append(append(request, byte(len(password))), password...)
	if _, err = conn.Write(request); err != nil {
		return fail(err)
	}
	if _, err = io.ReadFull(conn, reply); err != nil {
		return fail(err)
	}
	if reply[1] != 0 {
		conn.Close()
		return nil, reply[1], nil
	}
	host, portText, _ := net.SplitHostPort(target)
	port, _ := strconv.Atoi(portText)
	ip := net.ParseIP(host).To4()
	connect := append([]byte{5, 1, 0, 1}, ip...)
	connect = append(connect, byte(port>>8), byte(port))
	if _, err = conn.Write(connect); err != nil {
		return fail(err)
	}
	head := make([]byte, 10)
	if _, err = io.ReadFull(conn, head); err != nil || head[1] != 0 {
		return fail(fmt.Errorf("connect reply %v: %v", head, err))
	}
	return conn, 0, nil
}

func tunnelGet(conn net.Conn, host string) (int, error) {
	if _, err := io.WriteString(conn, "GET / HTTP/1.1\r\nHost: "+host+"\r\nConnection: keep-alive\r\n\r\n"); err != nil {
		return 0, err
	}
	response, err := http.ReadResponse(bufio.NewReader(conn), nil)
	if err != nil {
		return 0, err
	}
	response.Body.Close()
	return response.StatusCode, nil
}

// plainProxyGets sends count absolute-form GETs over one keep-alive proxy
// connection, each with Proxy-Authorization, and expects 204 for each.
func plainProxyGets(endpoint localproxy.Endpoint, username, password, url string, count int) error {
	conn, err := net.DialTimeout("tcp", proxyAddress(endpoint), time.Second)
	if err != nil {
		return err
	}
	defer conn.Close()
	_ = conn.SetDeadline(time.Now().Add(5 * time.Second))
	authorization := base64.StdEncoding.EncodeToString([]byte(username + ":" + password))
	reader := bufio.NewReader(conn)
	host := strings.TrimPrefix(url, "http://")
	for i := 0; i < count; i++ {
		request := "GET " + url + "/ HTTP/1.1\r\nHost: " + host + "\r\nProxy-Authorization: Basic " + authorization + "\r\nProxy-Connection: keep-alive\r\n\r\n"
		if _, err = io.WriteString(conn, request); err != nil {
			return err
		}
		response, err := http.ReadResponse(reader, nil)
		if err != nil {
			return fmt.Errorf("request %d: %w", i, err)
		}
		response.Body.Close()
		if response.StatusCode != http.StatusNoContent {
			return fmt.Errorf("request %d: %s", i, response.Status)
		}
	}
	return nil
}

func waitForConnectionNode(t *testing.T, core *Core, nodeID string) {
	t.Helper()
	deadline := time.Now().Add(3 * time.Second)
	for time.Now().Before(deadline) {
		for _, connection := range core.Connections() {
			if connection.NodeID == nodeID {
				return
			}
		}
		time.Sleep(20 * time.Millisecond)
	}
	t.Fatalf("no active connection attributed to %s: %#v", nodeID, core.Connections())
}

// TestSharedLocalProxyRoutesByUsername runs two nodes behind one loopback
// port: the username picks the node for HTTP and SOCKS5, traffic is counted
// and attributed per node, and bad credentials or removed nodes are rejected.
func TestSharedLocalProxyRoutesByUsername(t *testing.T) {
	target := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) { w.WriteHeader(http.StatusNoContent) }))
	defer target.Close()
	targetAddress := strings.TrimPrefix(target.URL, "http://")
	serverPort := startShadowsocksServer(t)
	deadPort := freePort(t)
	nodes := []profile.Node{
		{ID: "alpha-1", EntryKey: "cn-optimized", Capabilities: profile.Capabilities{TCP: true}, Ingresses: []profile.Ingress{localSSIngress(profile.IngressRolePrimary, "a0", 0, serverPort)}},
		{ID: "beta-2", EntryKey: "cn-optimized", Capabilities: profile.Capabilities{TCP: true}, Ingresses: []profile.Ingress{
			localSSIngress(profile.IngressRolePrimary, "b0", 0, deadPort),
			localSSIngress(profile.IngressRoleBackup, "b1", 1, serverPort),
		}},
	}
	p := &profile.Profile{
		SchemaVersion: profile.CurrentSchemaVersion, Revision: "shared-1", ExpiresAt: time.Now().Add(time.Hour), Nodes: nodes,
		Selection: profile.Selection{Mode: "manual", DefaultNodeID: "alpha-1"},
		// A catch-all REJECT proves local proxy traffic never falls through
		// to profile rules.
		Routing: profile.Routing{Final: profile.RoutingAction{Type: "reject"}},
	}
	platform := profile.PlatformCapabilities{Platform: "macos", LocalProxy: profile.LocalProxyCapabilities{Enabled: true, Listen: "127.0.0.1"}, LogLevel: "error"}
	core := newLocalProxyTestCore(t, platform)
	if _, err := core.ApplyProfile(p, time.Now()); err != nil {
		t.Fatal(err)
	}
	if err := core.Start(); err != nil {
		t.Fatal(err)
	}
	defer core.Stop()

	endpoints := core.LocalProxyEndpoints()
	if len(endpoints) != 2 || endpoints[0].Port != endpoints[1].Port || endpoints[0].Password != endpoints[1].Password {
		t.Fatalf("endpoints not shared: %#v", endpoints)
	}
	alpha, beta := endpoints[0], endpoints[1]
	// Usernames are the short "<prefix>-<alias>" logins, not "<prefix>-<nodeID>".
	prefix, _, _ := localproxy.ParseUsername(alpha.Username)
	aliases := localproxy.Aliases([]string{"alpha-1", "beta-2"})
	if alpha.Username != localproxy.Login(prefix, aliases["alpha-1"]) || beta.Username != localproxy.Login(prefix, aliases["beta-2"]) ||
		len(alpha.Username) != 12 || len(beta.Username) != 12 {
		t.Fatalf("usernames: %q %q", alpha.Username, beta.Username)
	}
	for _, id := range []string{"alpha-1", "beta-2"} {
		if credential, err := core.LocalProxyCredential(id); err != nil || credential.Username != localproxy.Login(prefix, aliases[id]) {
			t.Fatalf("credential %s: %#v %v", id, credential, err)
		}
	}
	for _, metadata := range core.LocalProxyMetadata() {
		if metadata.Port != alpha.Port || metadata.Listen != "127.0.0.1" {
			t.Fatalf("metadata: %#v", metadata)
		}
	}

	// HTTP CONNECT as alpha, SOCKS5 CONNECT as beta (via its backup ingress).
	// Logins are case-insensitive: alpha is sent in upper case, beta mixed.
	alphaConn, status, err := httpConnect(alpha, strings.ToUpper(alpha.Username), alpha.Password, targetAddress)
	if err != nil || !strings.HasPrefix(status, "200") {
		t.Fatalf("alpha CONNECT: %q %v", status, err)
	}
	defer alphaConn.Close()
	if code, err := tunnelGet(alphaConn, targetAddress); err != nil || code != http.StatusNoContent {
		t.Fatalf("alpha request: %d %v", code, err)
	}
	waitForConnectionNode(t, core, "alpha-1")
	betaConn, socksStatus, err := socks5Connect(beta, strings.ToUpper(beta.Username[:3])+beta.Username[3:], beta.Password, targetAddress)
	if err != nil || socksStatus != 0 {
		t.Fatalf("beta SOCKS5: %d %v", socksStatus, err)
	}
	defer betaConn.Close()
	if code, err := tunnelGet(betaConn, targetAddress); err != nil || code != http.StatusNoContent {
		t.Fatalf("beta request: %d %v", code, err)
	}
	waitForConnectionNode(t, core, "beta-2")
	if traffic := core.Traffic(); traffic.UploadBytes == 0 || traffic.DownloadBytes == 0 {
		t.Fatalf("traffic not counted: %#v", traffic)
	}
	for _, id := range []string{"alpha-1", "beta-2"} {
		if result, err := core.ProbeAvailability(t.Context(), id, target.URL, 5*time.Second); err != nil || !result.Success {
			t.Fatalf("availability %s: %v %#v", id, err, result)
		}
	}

	// Plain (non-CONNECT) HTTP proxying re-checks every keep-alive request;
	// a mixed-case login keeps working for the whole connection.
	if err := plainProxyGets(alpha, strings.ToUpper(alpha.Username), alpha.Password, target.URL, 2); err != nil {
		t.Fatalf("plain HTTP keep-alive: %v", err)
	}

	// Rejections: wrong password, unknown alias, foreign prefix, the full
	// node ID (internal route key, not a login), no auth.
	rejected := map[string][2]string{
		"wrong password": {alpha.Username, alpha.Password + "x"},
		"empty password": {alpha.Username, ""},
		"unknown alias":  {localproxy.Login(prefix, "zzzzzz"), alpha.Password},
		"other prefix":   {localproxy.Login("zzzzz", aliases["alpha-1"]), alpha.Password},
		"full node id":   {localproxy.FormatUsername(prefix, "alpha-1"), alpha.Password},
	}
	for name, credentials := range rejected {
		if conn, status, err := httpConnect(alpha, credentials[0], credentials[1], targetAddress); err != nil || !strings.HasPrefix(status, "407") {
			if conn != nil {
				conn.Close()
			}
			t.Fatalf("HTTP %s: %q %v", name, status, err)
		}
		if credentials[1] == "" {
			continue // RFC 1929 cannot carry an empty password portably
		}
		if _, socksStatus, err := socks5Connect(alpha, credentials[0], credentials[1], targetAddress); err != nil || socksStatus == 0 {
			t.Fatalf("SOCKS5 %s: %d %v", name, socksStatus, err)
		}
	}
	if err := assertHTTPAuthChallenge(alpha); err != nil {
		t.Fatal(err)
	}

	// Removing beta from the profile rejects its username without a crash;
	// alpha keeps the same port and credentials.
	next := *p
	next.Revision = "shared-2"
	next.Nodes = []profile.Node{cloneNode(nodes[0])}
	if applied, err := core.ApplyProfile(&next, time.Now()); err != nil || !applied {
		t.Fatalf("hot reload: %v", err)
	}
	after := core.LocalProxyEndpoints()
	if len(after) != 1 || after[0] != alpha {
		t.Fatalf("endpoints after removal: %#v", after)
	}
	if _, err := core.LocalProxyCredential("beta-2"); err == nil {
		t.Fatal("removed node still has a credential")
	}
	if conn, status, err := httpConnect(alpha, beta.Username, beta.Password, targetAddress); err != nil || !strings.HasPrefix(status, "407") {
		if conn != nil {
			conn.Close()
		}
		t.Fatalf("removed node over HTTP: %q %v", status, err)
	}
	if _, socksStatus, err := socks5Connect(alpha, beta.Username, beta.Password, targetAddress); err != nil || socksStatus == 0 {
		t.Fatalf("removed node over SOCKS5: %d %v", socksStatus, err)
	}
	conn, status, err := httpConnect(alpha, alpha.Username, alpha.Password, targetAddress)
	if err != nil || !strings.HasPrefix(status, "200") {
		t.Fatalf("alpha after reload: %q %v", status, err)
	}
	conn.Close()
}

// TestSharedLocalProxyConnectAuthFailureChallenges runs the real core: a
// CONNECT without Proxy-Authorization, with a wrong password or for an unknown
// user reads back a 407 Basic challenge and then a clean EOF (browsers need
// the challenge to prompt), and a correct CONNECT still tunnels.
func TestSharedLocalProxyConnectAuthFailureChallenges(t *testing.T) {
	target := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) { w.WriteHeader(http.StatusNoContent) }))
	defer target.Close()
	targetAddress := strings.TrimPrefix(target.URL, "http://")
	serverPort := startShadowsocksServer(t)
	p := &profile.Profile{
		SchemaVersion: profile.CurrentSchemaVersion, Revision: "challenge-1", ExpiresAt: time.Now().Add(time.Hour),
		Nodes: []profile.Node{{ID: "alpha-1", EntryKey: "cn-optimized", Capabilities: profile.Capabilities{TCP: true},
			Ingresses: []profile.Ingress{localSSIngress(profile.IngressRolePrimary, "a0", 0, serverPort)}}},
		Selection: profile.Selection{Mode: "manual", DefaultNodeID: "alpha-1"},
		Routing:   profile.Routing{Final: profile.RoutingAction{Type: "reject"}},
	}
	platform := profile.PlatformCapabilities{Platform: "windows", LocalProxy: profile.LocalProxyCapabilities{Enabled: true, Listen: "127.0.0.1"}, LogLevel: "error"}
	core := newLocalProxyTestCore(t, platform)
	if _, err := core.ApplyProfile(p, time.Now()); err != nil {
		t.Fatal(err)
	}
	if err := core.Start(); err != nil {
		t.Fatal(err)
	}
	defer core.Stop()
	endpoints := core.LocalProxyEndpoints()
	if len(endpoints) != 1 {
		t.Fatalf("endpoints: %#v", endpoints)
	}
	alpha := endpoints[0]
	prefix, _, _ := localproxy.ParseUsername(alpha.Username)
	authorization := func(username, password string) string {
		return "Proxy-Authorization: Basic " + base64.StdEncoding.EncodeToString([]byte(username+":"+password)) + "\r\n"
	}
	head := "CONNECT " + targetAddress + " HTTP/1.1\r\nHost: " + targetAddress + "\r\n"
	for name, request := range map[string]string{
		"no auth":        head + "\r\n",
		"wrong password": head + authorization(alpha.Username, alpha.Password+"x") + "\r\n",
		"unknown user":   head + authorization(localproxy.Login(prefix, "zzzzzz"), alpha.Password) + "\r\n",
		"full node id":   head + authorization(localproxy.FormatUsername(prefix, "alpha-1"), alpha.Password) + "\r\n",
	} {
		conn, err := net.DialTimeout("tcp", proxyAddress(alpha), time.Second)
		if err != nil {
			t.Fatal(err)
		}
		_ = conn.SetDeadline(time.Now().Add(5 * time.Second))
		if _, err = io.WriteString(conn, request); err != nil {
			t.Fatalf("%s: %v", name, err)
		}
		reader := bufio.NewReader(conn)
		response, err := http.ReadResponse(reader, nil)
		if err != nil {
			t.Fatalf("%s: no 407: %v", name, err)
		}
		if response.StatusCode != http.StatusProxyAuthRequired || !strings.HasPrefix(response.Header.Get("Proxy-Authenticate"), `Basic realm="ppvpn"`) ||
			response.ContentLength != 0 || !response.Close {
			t.Fatalf("%s: %q %v", name, response.Status, response.Header)
		}
		if n, err := reader.Read(make([]byte, 1)); n != 0 || err != io.EOF {
			t.Fatalf("%s: after 407: %d %v", name, n, err)
		}
		conn.Close()
	}

	conn, status, err := httpConnect(alpha, alpha.Username, alpha.Password, targetAddress)
	if err != nil || !strings.HasPrefix(status, "200") {
		t.Fatalf("authorized CONNECT: %q %v", status, err)
	}
	defer conn.Close()
	if code, err := tunnelGet(conn, targetAddress); err != nil || code != http.StatusNoContent {
		t.Fatalf("tunnel request: %d %v", code, err)
	}
}
