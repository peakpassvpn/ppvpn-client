// Command relaytime relays TCP connections from -listen to -target,
// unchanged, and times each one's flights: a flight is the data one side
// sends before the other answers. Placed between an engine and the fake
// node it shows where a new connection's setup goes (the engine's
// ClientHello, the node's answer, the engine's Finished and first data,
// the node's first data back). Loopback only.
//
// It prints one JSON line per connection when it ends: when it was
// accepted; each flight's direction (c: from the listen side, s: from the
// target), start in nanoseconds from the accept, and bytes; and, reading
// the bytes as TLS records, each record's direction, arrival, content type
// and length, and whether the ClientHello offers to resume a session
// (pre_shared_key or session_ticket extension).
package main

import (
	"encoding/json"
	"flag"
	"io"
	"log"
	"net"
	"os"
	"sync"
	"time"
)

type flight struct {
	Dir   string `json:"dir"`
	AtNs  int64  `json:"at_ns"`
	Bytes int    `json:"bytes"`
}

type tlsRecord struct {
	Dir    string `json:"dir"`
	AtNs   int64  `json:"at_ns"`
	Type   int    `json:"type"`
	Length int    `json:"len"`
}

type record struct {
	AcceptedUnixNs int64       `json:"accepted_unix_ns"`
	DialNs         int64       `json:"dial_ns"`
	Flights        []flight    `json:"flights"`
	Records        []tlsRecord `json:"records"`
	// The ClientHello's offer to resume: TLS 1.3 pre_shared_key (41),
	// TLS 1.2 session_ticket (35) with a ticket in it.
	OffersPSK    bool `json:"offers_psk"`
	OffersTicket bool `json:"offers_ticket"`
}

// records splits one direction's bytes into TLS records, as they arrive.
type records struct {
	pending []byte // a record header not whole yet
	left    int    // bytes of the current record still to come
	hello   []byte // the first record's body, while it is the ClientHello
	first   bool
}

// timeline keeps the flights of one connection: a read from the other side
// than the last one starts a new flight.
type timeline struct {
	mu          sync.Mutex
	start       time.Time
	flights     []flight
	records     []tlsRecord
	parse       map[string]*records
	psk, ticket bool
}

func (t *timeline) add(dir string, data []byte) {
	t.mu.Lock()
	defer t.mu.Unlock()
	at := time.Since(t.start).Nanoseconds()
	if last := len(t.flights) - 1; last >= 0 && t.flights[last].Dir == dir {
		t.flights[last].Bytes += len(data)
	} else if len(t.flights) < 16 {
		t.flights = append(t.flights, flight{Dir: dir, AtNs: at, Bytes: len(data)})
	}
	r := t.parse[dir]
	for len(data) > 0 && len(t.records) < 32 {
		if r.left > 0 {
			n := min(r.left, len(data))
			if r.first && dir == "c" {
				r.hello = append(r.hello, data[:n]...)
			}
			r.left -= n
			data = data[n:]
			if r.left == 0 && r.first && dir == "c" {
				t.psk, t.ticket = helloOffers(r.hello)
				r.first, r.hello = false, nil
			}
			continue
		}
		need := 5 - len(r.pending)
		n := min(need, len(data))
		r.pending = append(r.pending, data[:n]...)
		data = data[n:]
		if len(r.pending) < 5 {
			break
		}
		typ, length := int(r.pending[0]), int(r.pending[3])<<8|int(r.pending[4])
		t.records = append(t.records, tlsRecord{Dir: dir, AtNs: at, Type: typ, Length: length})
		r.pending, r.left = r.pending[:0], length
		if r.left == 0 {
			r.first = false
		}
	}
}

// helloOffers reads a ClientHello's extensions: whether it offers a TLS 1.3
// pre-shared key, and a TLS 1.2 session ticket that is not empty.
func helloOffers(body []byte) (psk, ticket bool) {
	// Handshake header (4), version (2), random (32).
	if len(body) < 38 || body[0] != 1 {
		return false, false
	}
	i := 38
	skip := func(width int) bool {
		if i+width > len(body) {
			return false
		}
		n := 0
		for k := 0; k < width; k++ {
			n = n<<8 | int(body[i+k])
		}
		i += width + n
		return i <= len(body)
	}
	// Session id (1), cipher suites (2), compression methods (1).
	if !skip(1) || !skip(2) || !skip(1) || i+2 > len(body) {
		return false, false
	}
	end := i + 2 + (int(body[i])<<8 | int(body[i+1]))
	i += 2
	for i+4 <= end && i+4 <= len(body) {
		typ, n := int(body[i])<<8|int(body[i+1]), int(body[i+2])<<8|int(body[i+3])
		switch {
		case typ == 41:
			psk = true
		case typ == 35 && n > 0:
			ticket = true
		}
		i += 4 + n
	}
	return psk, ticket
}

func pipe(dst, src net.Conn, t *timeline, dir string, done *sync.WaitGroup) {
	defer done.Done()
	buf := make([]byte, 64<<10)
	for {
		n, err := src.Read(buf)
		if n > 0 {
			t.add(dir, buf[:n])
			if _, werr := dst.Write(buf[:n]); werr != nil {
				break
			}
		}
		if err != nil {
			break
		}
	}
	if tcp, ok := dst.(*net.TCPConn); ok {
		_ = tcp.CloseWrite()
	}
}

func main() {
	listen := flag.String("listen", "127.0.0.1:0", "where to listen")
	target := flag.String("target", "", "where to relay to (required)")
	flag.Parse()
	if *target == "" {
		log.Fatal("-target is required")
	}
	listener, err := net.Listen("tcp", *listen)
	if err != nil {
		log.Fatal(err)
	}
	_ = json.NewEncoder(os.Stdout).Encode(map[string]int{"port": listener.Addr().(*net.TCPAddr).Port})
	var out sync.Mutex
	encoder := json.NewEncoder(os.Stdout)
	go func() { _, _ = io.Copy(io.Discard, os.Stdin); os.Exit(0) }()
	for {
		client, err := listener.Accept()
		if err != nil {
			return
		}
		go func() {
			defer client.Close()
			t := &timeline{start: time.Now(), parse: map[string]*records{
				"c": {first: true}, "s": {},
			}}
			accepted := t.start.UnixNano()
			server, err := net.Dial("tcp", *target)
			if err != nil {
				return
			}
			defer server.Close()
			for _, c := range []net.Conn{client, server} {
				if tcp, ok := c.(*net.TCPConn); ok {
					_ = tcp.SetNoDelay(true)
				}
			}
			dial := time.Since(t.start).Nanoseconds()
			var done sync.WaitGroup
			done.Add(2)
			go pipe(server, client, t, "c", &done)
			go pipe(client, server, t, "s", &done)
			done.Wait()
			t.mu.Lock()
			rec := record{AcceptedUnixNs: accepted, DialNs: dial, Flights: t.flights, Records: t.records,
				OffersPSK: t.psk, OffersTicket: t.ticket}
			t.mu.Unlock()
			out.Lock()
			_ = encoder.Encode(rec)
			out.Unlock()
		}()
	}
}
