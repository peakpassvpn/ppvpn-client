//go:build darwin || linux

package probe

import (
	"bytes"
	"context"
	"crypto/rand"
	"encoding/binary"
	"errors"
	"net"
	"net/netip"
	"syscall"
	"time"

	"golang.org/x/net/icmp"
	"golang.org/x/net/ipv4"
	"golang.org/x/net/ipv6"
)

const (
	protocolICMP     = 1
	protocolIPv6ICMP = 58
)

// Ping sends one ICMP echo request over an unprivileged datagram ICMP socket
// (SOCK_DGRAM + IPPROTO_ICMP[V6]). macOS/iOS allow these for every user;
// Linux/Android allow them for groups listed in net.ipv4.ping_group_range
// (the default on Android and most desktop distributions). No raw socket or
// elevated privilege is ever requested; when the kernel refuses the socket the
// probe reports ICMP_UNSUPPORTED.
func Ping(ctx context.Context, addr netip.Addr, timeout time.Duration) (time.Duration, error) {
	addr = addr.Unmap()
	network, listen, proto := "udp4", "0.0.0.0", protocolICMP
	var request icmp.Type = ipv4.ICMPTypeEcho
	if addr.Is6() {
		network, listen, proto = "udp6", "::", protocolIPv6ICMP
		request = ipv6.ICMPTypeEchoRequest
	}
	conn, err := icmp.ListenPacket(network, listen)
	if err != nil {
		return 0, &Error{Code: socketErrorCode(err), Err: err}
	}
	defer conn.Close()

	deadline := time.Now().Add(timeout)
	if d, ok := ctx.Deadline(); ok && d.Before(deadline) {
		deadline = d
	}
	if err = conn.SetDeadline(deadline); err != nil {
		return 0, &Error{Code: CodeICMPFailed, Err: err}
	}
	stop := context.AfterFunc(ctx, func() { _ = conn.SetDeadline(time.Unix(1, 0)) })
	defer stop()

	var random [18]byte
	if _, err = rand.Read(random[:]); err != nil {
		return 0, &Error{Code: CodeICMPFailed, Err: err}
	}
	// Linux rewrites the echo identifier to the socket's port, so replies are
	// matched on sequence number, source address and a random payload token.
	id := int(binary.BigEndian.Uint16(random[0:2]))
	seq := int(binary.BigEndian.Uint16(random[2:4]))
	token := random[2:]
	message := icmp.Message{Type: request, Body: &icmp.Echo{ID: id, Seq: seq, Data: token}}
	packet, err := message.Marshal(nil)
	if err != nil {
		return 0, &Error{Code: CodeICMPFailed, Err: err}
	}
	destination := &net.UDPAddr{IP: addr.AsSlice(), Zone: addr.Zone()}
	started := time.Now()
	if _, err = conn.WriteTo(packet, destination); err != nil {
		return 0, ioError(ctx, err)
	}
	buffer := make([]byte, 1500)
	for {
		n, peer, err := conn.ReadFrom(buffer)
		if err != nil {
			return 0, ioError(ctx, err)
		}
		rtt := time.Since(started)
		reply, err := icmp.ParseMessage(proto, buffer[:n])
		if err != nil {
			continue
		}
		switch reply.Type {
		case ipv4.ICMPTypeEchoReply, ipv6.ICMPTypeEchoReply:
			echo, ok := reply.Body.(*icmp.Echo)
			if ok && echo.Seq == seq && bytes.Equal(echo.Data, token) && samePeer(peer, addr) {
				return rtt, nil
			}
		case ipv4.ICMPTypeDestinationUnreachable, ipv6.ICMPTypeDestinationUnreachable:
			if body, ok := reply.Body.(*icmp.DstUnreach); ok && quotesEcho(body.Data, addr.Is6(), seq) {
				return 0, &Error{Code: CodeICMPUnreachable}
			}
		}
	}
}

func samePeer(peer net.Addr, want netip.Addr) bool {
	var ip net.IP
	switch a := peer.(type) {
	case *net.UDPAddr:
		ip = a.IP
	case *net.IPAddr:
		ip = a.IP
	default:
		return false
	}
	got, ok := netip.AddrFromSlice(ip)
	return ok && got.Unmap() == want.WithZone("")
}

// quotesEcho reports whether an ICMP error quotes our echo request.
func quotesEcho(data []byte, v6 bool, seq int) bool {
	offset := 40
	if !v6 {
		if len(data) < 1 {
			return false
		}
		offset = int(data[0]&0x0f) * 4
	}
	if len(data) < offset+8 {
		return false
	}
	return int(binary.BigEndian.Uint16(data[offset+6:offset+8])) == seq
}

func socketErrorCode(err error) string {
	switch {
	case errors.Is(err, syscall.EACCES), errors.Is(err, syscall.EPERM),
		errors.Is(err, syscall.EPROTONOSUPPORT), errors.Is(err, syscall.EAFNOSUPPORT),
		errors.Is(err, syscall.ESOCKTNOSUPPORT):
		return CodeICMPUnsupported
	default:
		return CodeICMPFailed
	}
}

func ioError(ctx context.Context, err error) error {
	if ctxErr := ctx.Err(); ctxErr != nil {
		if errors.Is(ctxErr, context.DeadlineExceeded) {
			return &Error{Code: CodeICMPTimeout, Err: ctxErr}
		}
		return ctxErr
	}
	var ne net.Error
	if errors.As(err, &ne) && ne.Timeout() {
		return &Error{Code: CodeICMPTimeout, Err: err}
	}
	switch {
	case errors.Is(err, syscall.EHOSTUNREACH), errors.Is(err, syscall.ENETUNREACH),
		errors.Is(err, syscall.ECONNREFUSED), errors.Is(err, syscall.EHOSTDOWN),
		errors.Is(err, syscall.EADDRNOTAVAIL):
		return &Error{Code: CodeICMPUnreachable, Err: err}
	case errors.Is(err, syscall.EACCES), errors.Is(err, syscall.EPERM):
		return &Error{Code: CodeICMPUnsupported, Err: err}
	default:
		return &Error{Code: CodeICMPFailed, Err: err}
	}
}
