package probe

import (
	"context"
	"errors"
	"net/netip"
	"testing"
	"time"
)

// TestPingLoopback exercises the real unprivileged ICMP implementation. It is
// skipped where the host forbids unprivileged ICMP (e.g. Linux without a
// matching net.ipv4.ping_group_range), which is itself reported correctly.
func TestPingLoopback(t *testing.T) {
	for _, target := range []string{"127.0.0.1", "::1"} {
		t.Run(target, func(t *testing.T) {
			rtt, err := Ping(context.Background(), netip.MustParseAddr(target), 2*time.Second)
			var coded *Error
			if errors.As(err, &coded) && coded.Code == CodeICMPUnsupported {
				t.Skipf("unprivileged ICMP unavailable: %v", err)
			}
			if err != nil && target == "::1" && errors.As(err, &coded) && coded.Code == CodeICMPUnreachable {
				t.Skipf("IPv6 loopback unavailable: %v", err)
			}
			if err != nil {
				t.Fatal(err)
			}
			// Windows reports whole milliseconds (IcmpSendEcho), so a
			// loopback echo can take 0.
			if rtt < 0 || rtt > 2*time.Second {
				t.Fatalf("rtt %v", rtt)
			}
		})
	}
}

func TestPingTimeoutAndCancel(t *testing.T) {
	// 192.0.2.0/24 (TEST-NET-1) is never routed; the echo must time out or be
	// reported unreachable, never succeed.
	_, err := Ping(context.Background(), netip.MustParseAddr("192.0.2.1"), 200*time.Millisecond)
	var coded *Error
	if !errors.As(err, &coded) {
		t.Fatalf("expected coded error, got %v", err)
	}
	if coded.Code == CodeICMPUnsupported {
		t.Skip("unprivileged ICMP unavailable")
	}
	if coded.Code != CodeICMPTimeout && coded.Code != CodeICMPUnreachable {
		t.Fatalf("unexpected code %s", coded.Code)
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	started := time.Now()
	_, err = Ping(ctx, netip.MustParseAddr("192.0.2.1"), 5*time.Second)
	if err == nil || time.Since(started) > time.Second {
		t.Fatalf("cancel not honored: %v after %v", err, time.Since(started))
	}
}
