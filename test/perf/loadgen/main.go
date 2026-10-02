// Command loadgen drives the performance checks' fixed loads through an
// engine's local proxy (HTTP CONNECT with a node user) to fakenode's echo
// sink, on loopback only:
//
//	stream: -conns connections sending -rate-mbit in total, paced, for
//	        -duration; the sink echoes, so as much comes back.
//	churn:  -churn-rate new connections per second, each sending and
//	        reading back -churn-bytes, for -duration.
//
// It prints one JSON line: bytes sent and received, connections that
// completed and that failed.
package main

import (
	"bufio"
	"encoding/base64"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"log"
	"net"
	"net/http"
	"os"
	"sync"
	"sync/atomic"
	"time"
)

type result struct {
	BytesSent      int64 `json:"bytes_sent"`
	BytesReceived  int64 `json:"bytes_received"`
	Connections    int64 `json:"connections"`
	FailedConnects int64 `json:"failed_connections"`
}

func main() {
	proxy := flag.String("proxy", "", "local proxy host:port")
	user := flag.String("user", "", "proxy username")
	pass := flag.String("pass", "", "proxy password")
	target := flag.String("target", "", "sink host:port")
	mode := flag.String("mode", "stream", "stream or churn")
	conns := flag.Int("conns", 8, "stream: connections")
	rate := flag.Float64("rate-mbit", 50, "stream: total send rate, Mbit/s")
	churnRate := flag.Int("churn-rate", 200, "churn: new connections per second")
	churnBytes := flag.Int("churn-bytes", 4096, "churn: bytes each connection sends and reads back")
	duration := flag.Duration("duration", 30*time.Second, "how long the load runs")
	flag.Parse()
	if *proxy == "" || *target == "" {
		log.Fatal("-proxy and -target are required")
	}
	dial := func() (net.Conn, error) { return connect(*proxy, *user, *pass, *target) }
	var r result
	switch *mode {
	case "stream":
		stream(dial, *conns, *rate, *duration, &r)
	case "churn":
		churn(dial, *churnRate, *churnBytes, *duration, &r)
	default:
		log.Fatalf("unknown mode %q", *mode)
	}
	_ = json.NewEncoder(os.Stdout).Encode(r)
	if r.Connections == 0 {
		os.Exit(1)
	}
}

func connect(proxy, user, pass, target string) (net.Conn, error) {
	conn, err := net.DialTimeout("tcp", proxy, 5*time.Second)
	if err != nil {
		return nil, err
	}
	auth := base64.StdEncoding.EncodeToString([]byte(user + ":" + pass))
	fmt.Fprintf(conn, "CONNECT %s HTTP/1.1\r\nHost: %s\r\nProxy-Authorization: Basic %s\r\n\r\n", target, target, auth)
	reader := bufio.NewReader(conn)
	response, err := http.ReadResponse(reader, nil)
	if err != nil {
		conn.Close()
		return nil, err
	}
	if response.StatusCode != http.StatusOK {
		conn.Close()
		return nil, fmt.Errorf("CONNECT: %s", response.Status)
	}
	if reader.Buffered() > 0 {
		conn.Close()
		return nil, errors.New("unexpected bytes after CONNECT")
	}
	return conn, nil
}

func stream(dial func() (net.Conn, error), conns int, rateMbit float64, duration time.Duration, r *result) {
	const chunk = 16 << 10
	perConn := rateMbit * 1e6 / 8 / float64(conns) // bytes per second
	interval := time.Duration(float64(time.Second) * chunk / perConn)
	deadline := time.Now().Add(duration)
	var wg sync.WaitGroup
	for range conns {
		wg.Add(1)
		go func() {
			defer wg.Done()
			conn, err := dial()
			if err != nil {
				atomic.AddInt64(&r.FailedConnects, 1)
				return
			}
			defer conn.Close()
			atomic.AddInt64(&r.Connections, 1)
			done := make(chan struct{})
			go func() {
				n, _ := io.Copy(io.Discard, conn)
				atomic.AddInt64(&r.BytesReceived, n)
				close(done)
			}()
			buf := make([]byte, chunk)
			ticker := time.NewTicker(interval)
			defer ticker.Stop()
			for time.Now().Before(deadline) {
				n, err := conn.Write(buf)
				atomic.AddInt64(&r.BytesSent, int64(n))
				if err != nil {
					break
				}
				<-ticker.C
			}
			// Let the echo drain, then close.
			if tcp, ok := conn.(*net.TCPConn); ok {
				_ = tcp.CloseWrite()
			}
			select {
			case <-done:
			case <-time.After(5 * time.Second):
			}
		}()
	}
	wg.Wait()
}

func churn(dial func() (net.Conn, error), rate, size int, duration time.Duration, r *result) {
	ticker := time.NewTicker(time.Second / time.Duration(rate))
	defer ticker.Stop()
	deadline := time.Now().Add(duration)
	var wg sync.WaitGroup
	payload := make([]byte, size)
	for time.Now().Before(deadline) {
		<-ticker.C
		wg.Add(1)
		go func() {
			defer wg.Done()
			conn, err := dial()
			if err != nil {
				atomic.AddInt64(&r.FailedConnects, 1)
				return
			}
			defer conn.Close()
			_ = conn.SetDeadline(time.Now().Add(10 * time.Second))
			n, err := conn.Write(payload)
			atomic.AddInt64(&r.BytesSent, int64(n))
			if err != nil {
				atomic.AddInt64(&r.FailedConnects, 1)
				return
			}
			got, err := io.ReadFull(conn, make([]byte, size))
			atomic.AddInt64(&r.BytesReceived, int64(got))
			if err != nil {
				atomic.AddInt64(&r.FailedConnects, 1)
				return
			}
			atomic.AddInt64(&r.Connections, 1)
		}()
	}
	wg.Wait()
}
