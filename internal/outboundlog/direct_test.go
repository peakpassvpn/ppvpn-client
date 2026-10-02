package outboundlog

import (
	"testing"
	"time"
)

func TestLimiterLogsOncePerWindowWithSuppressedCount(t *testing.T) {
	l := newLimiter(DirectLimit)
	start := time.Now()
	if ok, suppressed := l.allow("tcp a:1", start); !ok || suppressed != 0 {
		t.Fatalf("first failure: %v %d", ok, suppressed)
	}
	for i := 1; i <= 3; i++ {
		if ok, _ := l.allow("tcp a:1", start.Add(time.Duration(i)*time.Second)); ok {
			t.Fatalf("failure %d within the window was logged", i)
		}
	}
	if ok, _ := l.allow("tcp b:1", start.Add(time.Second)); !ok {
		t.Fatal("another destination is limited separately")
	}
	if ok, suppressed := l.allow("tcp a:1", start.Add(DirectLimit)); !ok || suppressed != 3 {
		t.Fatalf("after the window: %v suppressed=%d, want true 3", ok, suppressed)
	}
	if ok, suppressed := l.allow("tcp a:1", start.Add(3*DirectLimit)); !ok || suppressed != 0 {
		t.Fatalf("count resets once logged: %v %d", ok, suppressed)
	}
}

func TestLimiterForgetsOldDestinationsWhenFull(t *testing.T) {
	l := newLimiter(DirectLimit)
	start := time.Now()
	for i := range directLimiterSize {
		l.allow(string(rune(i))+":1", start)
	}
	l.allow("new:1", start.Add(DirectLimit))
	if len(l.seen) != 1 {
		t.Fatalf("remembered %d destinations, want only the new one", len(l.seen))
	}
}
