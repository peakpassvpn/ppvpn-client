package probe

import (
	"context"
	"errors"
	"fmt"
	"net"
	"strconv"
	"sync"
	"time"

	"github.com/peakpassvpn/ppvpn-core/profile"
)

type EntranceResult struct {
	NodeID     string    `json:"node_id"`
	ConnectMS  int64     `json:"connect_ms"`
	Success    bool      `json:"success"`
	ErrorCode  string    `json:"error_code,omitempty"`
	MeasuredAt time.Time `json:"measured_at"`
}
type DialContext func(context.Context, string, string) (net.Conn, error)

func Entrances(ctx context.Context, p *profile.Profile, timeout time.Duration, concurrency int, dial DialContext) ([]EntranceResult, error) {
	if err := profile.Validate(p, time.Now()); err != nil {
		return nil, err
	}
	if concurrency < 1 {
		concurrency = 4
	}
	if dial == nil {
		dial = (&net.Dialer{Timeout: timeout}).DialContext
	}
	results := make([]EntranceResult, len(p.Nodes))
	sem := make(chan struct{}, concurrency)
	var wg sync.WaitGroup
	for i, n := range p.Nodes {
		wg.Add(1)
		go func(i int, n profile.Node) {
			defer wg.Done()
			select {
			case sem <- struct{}{}:
			case <-ctx.Done():
				results[i] = failed(n.ID, "CANCELED")
				return
			}
			defer func() { <-sem }()
			started := time.Now()
			attempt := ctx
			if timeout > 0 {
				var cancel context.CancelFunc
				attempt, cancel = context.WithTimeout(ctx, timeout)
				defer cancel()
			}
			conn, err := dial(attempt, "tcp", net.JoinHostPort(n.Endpoint.IP, strconv.Itoa(int(n.Endpoint.Port))))
			at := time.Now()
			r := EntranceResult{NodeID: n.ID, ConnectMS: at.Sub(started).Milliseconds(), MeasuredAt: at}
			if err == nil {
				r.Success = true
				conn.Close()
			} else {
				r.ErrorCode = probeError(err)
			}
			results[i] = r
		}(i, n)
	}
	wg.Wait()
	return results, nil
}
func failed(id, code string) EntranceResult {
	return EntranceResult{NodeID: id, ErrorCode: code, MeasuredAt: time.Now()}
}
func probeError(err error) string {
	if errors.Is(err, context.Canceled) {
		return "CANCELED"
	}
	if errors.Is(err, context.DeadlineExceeded) {
		return "TIMEOUT"
	}
	var ne net.Error
	if errors.As(err, &ne) && ne.Timeout() {
		return "TIMEOUT"
	}
	return "CONNECT_FAILED"
}
func Address(n profile.Node) string { return fmt.Sprintf("%s:%d", n.Endpoint.IP, n.Endpoint.Port) }
