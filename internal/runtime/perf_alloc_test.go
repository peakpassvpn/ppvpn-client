package runtime

import (
	"bufio"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"net/http"
	"os"
	"os/exec"
	goruntime "runtime"
	"strconv"
	"sync"
	"testing"
	"time"

	"github.com/peakpassvpn/ppvpn-core/profile"
)

// TestPerfAllocations is the Go engine's allocation numbers for the
// performance checks (tools/perf): allocations while the local proxy carries
// data and new connections to test/perf/fakenode on loopback, read from the
// Go runtime. Only comparable with the same engine (a Rust engine counts with
// its allocator). The count includes this test's client, which reuses its
// buffers. Runs only with PPVPN_PERF_FAKENODE set to a built fakenode; prints
// one `PERF {...}` line per round.
func TestPerfAllocations(t *testing.T) {
	fakenodeBin := os.Getenv("PPVPN_PERF_FAKENODE")
	if fakenodeBin == "" {
		t.Skip("set PPVPN_PERF_FAKENODE to a built test/perf/fakenode")
	}
	rounds := 1
	if value := os.Getenv("PPVPN_PERF_ROUNDS"); value != "" {
		rounds, _ = strconv.Atoi(value)
	}
	dir := t.TempDir()
	node := exec.Command(fakenodeBin, "-dir", dir)
	stdin, _ := node.StdinPipe()
	stdout, _ := node.StdoutPipe()
	if err := node.Start(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { stdin.Close(); _ = node.Process.Kill(); _ = node.Wait() })
	var ports struct {
		SS          uint16 `json:"ss_port"`
		AnyTLS      uint16 `json:"anytls_port"`
		Sink        uint16 `json:"sink_port"`
		Certificate string `json:"certificate"`
	}
	if err := json.NewDecoder(stdout).Decode(&ports); err != nil {
		t.Fatal(err)
	}
	// Before anything loads the system roots: the fake node's certificate.
	t.Setenv("SSL_CERT_FILE", ports.Certificate)

	p := &profile.Profile{
		SchemaVersion: profile.CurrentSchemaVersion, Revision: "perf", ExpiresAt: time.Now().Add(time.Hour),
		Nodes: []profile.Node{
			{ID: "ss", EntryKey: "perf", Capabilities: profile.Capabilities{TCP: true}, Ingresses: []profile.Ingress{{
				Role: profile.IngressRolePrimary, EndpointKey: "ss-1", Protocol: profile.ProtocolShadowsocks,
				Endpoint:     profile.Endpoint{Domain: "localhost", Port: ports.SS},
				Credentials:  profile.Credentials{Shadowsocks: &profile.ShadowsocksCredentials{Method: "2022-blake3-aes-128-gcm", UserKey: "AAAAAAAAAAAAAAAAAAAAAA=="}},
				Capabilities: profile.Capabilities{TCP: true}}}},
			{ID: "anytls", EntryKey: "perf", Capabilities: profile.Capabilities{TCP: true}, Ingresses: []profile.Ingress{{
				Role: profile.IngressRolePrimary, EndpointKey: "anytls-1", Protocol: profile.ProtocolAnyTLS,
				Endpoint:     profile.Endpoint{Domain: "localhost", Port: ports.AnyTLS},
				Credentials:  profile.Credentials{AnyTLS: &profile.AnyTLSCredentials{Password: "perf-anytls-password"}},
				TLS:          &profile.TLS{ServerName: "localhost"},
				Capabilities: profile.Capabilities{TCP: true}}}},
		},
		Selection: profile.Selection{Mode: "manual", DefaultNodeID: "ss"},
		Routing:   profile.Routing{Final: profile.RoutingAction{Type: "proxy", Target: "selected"}},
	}
	platform := profile.PlatformCapabilities{Platform: "linux", LocalProxy: profile.LocalProxyCapabilities{Enabled: true, Listen: "127.0.0.1"}, LogLevel: "error"}
	core := newLocalProxyTestCore(t, platform)
	if _, err := core.ApplyProfile(p, time.Now()); err != nil {
		t.Fatal(err)
	}
	if err := core.Start(); err != nil {
		t.Fatal(err)
	}
	defer core.Stop()
	target := net.JoinHostPort("127.0.0.1", strconv.Itoa(int(ports.Sink)))
	for round := 1; round <= rounds; round++ {
		row := map[string]any{"profile": "alloc", "round": round}
		for _, nodeID := range []string{"ss", "anytls"} {
			endpoint, err := core.LocalProxyCredential(nodeID)
			if err != nil {
				t.Fatal(err)
			}
			dial := func() (net.Conn, error) {
				return perfConnect(net.JoinHostPort(endpoint.Listen, strconv.Itoa(int(endpoint.Port))), endpoint.Username, endpoint.Password, target)
			}
			const streamBytes = 64 << 20
			allocs, bytes := measureAllocations(func() { perfStream(t, dial, 8, streamBytes) })
			row[nodeID+".allocations_per_mib"] = float64(allocs) / (streamBytes >> 20)
			row[nodeID+".allocated_bytes_per_mib"] = float64(bytes) / (streamBytes >> 20)
			const connections = 500
			allocs, _ = measureAllocations(func() { perfChurn(t, dial, connections, 4096) })
			row[nodeID+".allocations_per_connection"] = float64(allocs) / connections
		}
		line, _ := json.Marshal(row)
		fmt.Println("PERF " + string(line))
	}
}

func measureAllocations(run func()) (uint64, uint64) {
	var before, after goruntime.MemStats
	goruntime.GC()
	goruntime.ReadMemStats(&before)
	run()
	goruntime.ReadMemStats(&after)
	return after.Mallocs - before.Mallocs, after.TotalAlloc - before.TotalAlloc
}

func perfConnect(proxy, user, pass, target string) (net.Conn, error) {
	conn, err := net.DialTimeout("tcp", proxy, 5*time.Second)
	if err != nil {
		return nil, err
	}
	auth := base64.StdEncoding.EncodeToString([]byte(user + ":" + pass))
	fmt.Fprintf(conn, "CONNECT %s HTTP/1.1\r\nHost: %s\r\nProxy-Authorization: Basic %s\r\n\r\n", target, target, auth)
	response, err := http.ReadResponse(bufio.NewReader(conn), nil)
	if err != nil || response.StatusCode != http.StatusOK {
		conn.Close()
		return nil, fmt.Errorf("CONNECT: %v %v", response, err)
	}
	return conn, nil
}

// perfStream sends total bytes over conns connections and reads the echo.
func perfStream(t *testing.T, dial func() (net.Conn, error), conns, total int) {
	t.Helper()
	var wg sync.WaitGroup
	for range conns {
		wg.Add(1)
		go func() {
			defer wg.Done()
			conn, err := dial()
			if err != nil {
				t.Error(err)
				return
			}
			defer conn.Close()
			share := total / conns
			done := make(chan struct{})
			go func() {
				_, _ = io.CopyN(io.Discard, conn, int64(share))
				close(done)
			}()
			buf := make([]byte, 32<<10)
			for sent := 0; sent < share; sent += len(buf) {
				if _, err := conn.Write(buf); err != nil {
					t.Error(err)
					return
				}
			}
			<-done
		}()
	}
	wg.Wait()
}

// perfChurn opens n connections, 32 at a time, each sending and reading back
// size bytes.
func perfChurn(t *testing.T, dial func() (net.Conn, error), n, size int) {
	t.Helper()
	slots := make(chan struct{}, 32)
	var wg sync.WaitGroup
	for range n {
		wg.Add(1)
		slots <- struct{}{}
		go func() {
			defer func() { <-slots; wg.Done() }()
			conn, err := dial()
			if err != nil {
				t.Error(err)
				return
			}
			defer conn.Close()
			payload := make([]byte, size)
			if _, err := conn.Write(payload); err != nil {
				t.Error(err)
				return
			}
			if _, err := io.ReadFull(conn, payload); err != nil {
				t.Error(err)
			}
		}()
	}
	wg.Wait()
}
