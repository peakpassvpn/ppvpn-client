package runtime

import (
	"bufio"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/peakpassvpn/ppvpn-core/localproxy"
	"github.com/peakpassvpn/ppvpn-core/profile"
)

func TestRealSingBoxRunsMultipleLocalProxies(t *testing.T) {
	platform := profile.PlatformCapabilities{Platform: "macos", LocalProxy: profile.LocalProxyCapabilities{Enabled: true, Listen: "127.0.0.1"}, LogLevel: "error"}
	core := NewWithLocalProxyState(platform, filepath.Join(t.TempDir(), "proxy-state.json"))
	p := testProfile("r1", "edge.example.com", "8.8.8.8")
	p.Nodes[0].Credentials.Shadowsocks.ServerKey = "AAAAAAAAAAAAAAAAAAAAAA=="
	second := p.Nodes[0]
	second.ID = "node-2"
	second.Endpoint.IP = "1.1.1.1"
	p.Nodes = append(p.Nodes, second)
	if _, err := core.ApplyProfile(p, time.Now()); err != nil {
		t.Fatal(err)
	}
	if err := core.Start(); err != nil {
		t.Fatal(err)
	}
	defer core.Stop()
	endpoints := core.LocalProxyEndpoints()
	if len(endpoints) != 2 {
		t.Fatalf("got %d endpoints", len(endpoints))
	}
	metadata := core.LocalProxyMetadata()
	if len(metadata) != len(endpoints) {
		t.Fatalf("got %d metadata records", len(metadata))
	}
	metadataJSON, err := json.Marshal(metadata)
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(string(metadataJSON), "username") || strings.Contains(string(metadataJSON), "password") {
		t.Fatalf("metadata leaked credentials: %s", metadataJSON)
	}
	for i := range metadata {
		if metadata[i].NodeID != endpoints[i].NodeID ||
			metadata[i].Listen != endpoints[i].Listen ||
			metadata[i].Port != endpoints[i].Port ||
			len(metadata[i].Protocols) != 2 ||
			metadata[i].Protocols[0] != "http" ||
			metadata[i].Protocols[1] != "socks5" ||
			!metadata[i].AuthRequired {
			t.Fatalf("metadata does not describe mixed endpoint: %#v", metadata[i])
		}
		credential, credentialErr := core.LocalProxyCredential(metadata[i].NodeID)
		if credentialErr != nil {
			t.Fatal(credentialErr)
		}
		if credential.NodeID != endpoints[i].NodeID ||
			credential.Listen != endpoints[i].Listen ||
			credential.Port != endpoints[i].Port ||
			credential.Username != endpoints[i].Username ||
			credential.Password != endpoints[i].Password {
			t.Fatalf("credential lookup mismatch: %#v", credential)
		}
	}
	if _, err = core.LocalProxyCredential("missing-node"); err == nil {
		t.Fatal("missing local proxy credential lookup succeeded")
	}
	for _, endpoint := range endpoints {
		conn, err := net.DialTimeout("tcp", net.JoinHostPort(endpoint.Listen, strconv.Itoa(int(endpoint.Port))), time.Second)
		if err != nil {
			t.Fatalf("%s: %v", endpoint.NodeID, err)
		}
		conn.Close()
	}
	if err := assertHTTPAuthChallenge(endpoints[0]); err != nil {
		t.Fatal(err)
	}
	if err := assertSOCKS5Auth(endpoints[1]); err != nil {
		t.Fatal(err)
	}
	migrated := testProfile("r2", "new-edge.example.com", "1.1.1.1")
	migrated.Nodes[0].Credentials.Shadowsocks.ServerKey = "AAAAAAAAAAAAAAAAAAAAAA=="
	second = migrated.Nodes[0]
	second.ID = "node-2"
	second.Endpoint.IP = "8.8.4.4"
	migrated.Nodes = append(migrated.Nodes, second)
	if applied, err := core.ApplyProfile(migrated, time.Now()); err != nil || !applied {
		t.Fatalf("hot reload: %v", err)
	}
	if core.Status().State != StateRunning || core.Status().Revision != "r2" {
		t.Fatalf("status: %#v", core.Status())
	}
	after := core.LocalProxyEndpoints()
	for i := range endpoints {
		if endpoints[i] != after[i] {
			t.Fatal("stable proxy endpoint changed during migration")
		}
	}
}

func dialLocal(endpoint localproxy.Endpoint) (net.Conn, error) {
	return net.DialTimeout("tcp", net.JoinHostPort(endpoint.Listen, strconv.Itoa(int(endpoint.Port))), time.Second)
}

func assertHTTPAuthChallenge(endpoint localproxy.Endpoint) error {
	conn, err := dialLocal(endpoint)
	if err != nil {
		return err
	}
	defer conn.Close()
	_ = conn.SetDeadline(time.Now().Add(time.Second))
	if _, err = conn.Write([]byte("CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n")); err != nil {
		return err
	}
	status, err := bufio.NewReader(conn).ReadString('\n')
	if err != nil {
		return err
	}
	if !strings.Contains(status, "407") {
		return fmt.Errorf("expected HTTP proxy authentication challenge, got %q", status)
	}
	return nil
}
func assertSOCKS5Auth(endpoint localproxy.Endpoint) error {
	conn, err := dialLocal(endpoint)
	if err != nil {
		return err
	}
	defer conn.Close()
	_ = conn.SetDeadline(time.Now().Add(time.Second))
	if _, err = conn.Write([]byte{5, 1, 2}); err != nil {
		return err
	}
	reply := make([]byte, 2)
	if _, err = io.ReadFull(conn, reply); err != nil {
		return err
	}
	if reply[0] != 5 || reply[1] != 2 {
		return fmt.Errorf("expected SOCKS5 username/password method, got %v", reply)
	}
	request := append([]byte{1, byte(len(endpoint.Username))}, []byte(endpoint.Username)...)
	request = append(request, byte(len(endpoint.Password)))
	request = append(request, []byte(endpoint.Password)...)
	if _, err = conn.Write(request); err != nil {
		return err
	}
	if _, err = io.ReadFull(conn, reply); err != nil {
		return err
	}
	if reply[1] != 0 {
		return fmt.Errorf("SOCKS5 credentials rejected")
	}
	return nil
}
