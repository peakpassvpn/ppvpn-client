package runtime

import (
	"net"
	"testing"
	"time"
)

type datagramTestConn struct {
	packet         []byte
	writes         [][]byte
	closed         bool
	readDeadlines  int
	writeDeadlines int
}

func (c *datagramTestConn) Read(buffer []byte) (int, error) {
	return copy(buffer, c.packet), nil
}
func (c *datagramTestConn) Write(data []byte) (int, error) {
	c.writes = append(c.writes, append([]byte(nil), data...))
	return len(data), nil
}
func (c *datagramTestConn) Close() error {
	c.closed = true
	return nil
}
func (c *datagramTestConn) LocalAddr() net.Addr         { return &net.UDPAddr{} }
func (c *datagramTestConn) RemoteAddr() net.Addr        { return &net.UDPAddr{} }
func (c *datagramTestConn) SetDeadline(time.Time) error { return nil }
func (c *datagramTestConn) SetReadDeadline(time.Time) error {
	c.readDeadlines++
	return nil
}
func (c *datagramTestConn) SetWriteDeadline(time.Time) error {
	c.writeDeadlines++
	return nil
}

func TestUDPReadNeverSilentlyTruncatesDatagram(t *testing.T) {
	transport := &datagramTestConn{packet: []byte("complete-datagram")}
	connection := &FlowConnection{conn: transport, network: "udp"}
	if data, err := connection.Read(4, 0); err == nil || data != nil {
		t.Fatalf("small caller buffer silently succeeded: %q %v", data, err)
	}

	transport = &datagramTestConn{packet: []byte("complete-datagram")}
	connection = &FlowConnection{conn: transport, network: "udp"}
	data, err := connection.Read(maxDatagramPayload, 0)
	if err != nil || string(data) != "complete-datagram" {
		t.Fatalf("complete datagram: %q %v", data, err)
	}
	if transport.readDeadlines != 0 {
		t.Fatalf("zero timeout installed %d read deadlines", transport.readDeadlines)
	}
}

func TestUDPWritePreservesOneCallPerDatagram(t *testing.T) {
	transport := &datagramTestConn{}
	connection := &FlowConnection{conn: transport, network: "udp"}
	if err := connection.Write([]byte("one"), 0); err != nil {
		t.Fatal(err)
	}
	if err := connection.Write([]byte("two"), 0); err != nil {
		t.Fatal(err)
	}
	if err := connection.Write([]byte{}, 0); err != nil {
		t.Fatal(err)
	}
	if len(transport.writes) != 3 ||
		string(transport.writes[0]) != "one" ||
		string(transport.writes[1]) != "two" ||
		len(transport.writes[2]) != 0 {
		t.Fatalf("datagram boundaries changed: %#v", transport.writes)
	}
	if err := connection.Write(make([]byte, maxDatagramWrite+1), 0); err == nil {
		t.Fatal("oversized datagram accepted")
	}
	if transport.writeDeadlines != 0 {
		t.Fatalf("zero timeout installed %d write deadlines", transport.writeDeadlines)
	}
}
