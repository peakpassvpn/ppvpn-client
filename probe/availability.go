package probe

import (
	"context"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"strconv"
	"time"

	"github.com/peakpassvpn/ppvpn-core/localproxy"
)

type AvailabilityResult struct {
	NodeID     string    `json:"node_id"`
	TotalMS    int64     `json:"total_ms"`
	Success    bool      `json:"success"`
	HTTPStatus int       `json:"http_status,omitempty"`
	ErrorCode  string    `json:"error_code,omitempty"`
	MeasuredAt time.Time `json:"measured_at"`
}

// Availability performs an end-to-end request through the node's authenticated
// local HTTP proxy, including proxy protocol and target connection. It is never
// reported as entrance/connect latency.
func Availability(ctx context.Context, endpoint localproxy.Endpoint, target string, timeout time.Duration) AvailabilityResult {
	started := time.Now()
	result := AvailabilityResult{NodeID: endpoint.NodeID}
	finish := func() { result.MeasuredAt = time.Now(); result.TotalMS = result.MeasuredAt.Sub(started).Milliseconds() }
	targetURL, err := url.Parse(target)
	if err != nil || targetURL.Scheme == "" || targetURL.Host == "" {
		result.ErrorCode = "TARGET_INVALID"
		finish()
		return result
	}
	proxyURL := &url.URL{Scheme: "http", Host: net.JoinHostPort(endpoint.Listen, strconv.Itoa(int(endpoint.Port))), User: url.UserPassword(endpoint.Username, endpoint.Password)}
	transport := &http.Transport{Proxy: http.ProxyURL(proxyURL), DialContext: (&net.Dialer{Timeout: timeout}).DialContext, ForceAttemptHTTP2: false}
	defer transport.CloseIdleConnections()
	client := &http.Client{Transport: transport, Timeout: timeout}
	request, err := http.NewRequestWithContext(ctx, http.MethodGet, target, nil)
	if err != nil {
		result.ErrorCode = "TARGET_INVALID"
		finish()
		return result
	}
	response, err := client.Do(request)
	if err != nil {
		result.ErrorCode = availabilityError(err)
		finish()
		return result
	}
	defer response.Body.Close()
	_, _ = io.Copy(io.Discard, io.LimitReader(response.Body, 64<<10))
	result.HTTPStatus = response.StatusCode
	result.Success = response.StatusCode >= 200 && response.StatusCode < 400
	if !result.Success {
		result.ErrorCode = "HTTP_STATUS"
	}
	finish()
	return result
}

func availabilityError(err error) string {
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
	return "PROXY_REQUEST_FAILED"
}
func AvailabilityAddress(endpoint localproxy.Endpoint) string {
	return fmt.Sprintf("%s:%d", endpoint.Listen, endpoint.Port)
}
