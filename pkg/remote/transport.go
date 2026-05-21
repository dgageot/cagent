package remote

import (
	"context"
	"errors"
	"log/slog"
	"net"
	"net/http"
	"net/url"
	"sync/atomic"
	"syscall"
	"time"

	"github.com/kofalt/go-memoize"

	"github.com/docker/docker-agent/pkg/desktop"
	socket "github.com/docker/docker-agent/pkg/desktop/socket"
)

var memoizer = memoize.NewMemoizer(1*time.Minute, 1*time.Minute)

// NewTransport returns an HTTP transport that uses Docker Desktop proxy if available.
// If the proxy becomes unavailable during the session, it automatically falls back
// to direct connections.
func NewTransport(ctx context.Context) http.RoundTripper {
	t, ok := http.DefaultTransport.(*http.Transport)
	if !ok {
		return http.DefaultTransport
	}
	transport := t.Clone()

	desktopRunning, err, _ := memoizer.Memoize("desktopRunning", func() (any, error) {
		return desktop.IsDockerDesktopRunning(context.Background()), nil
	})
	if err != nil {
		return transport
	}
	if running, ok := desktopRunning.(bool); ok && running {
		// Create a proxy transport
		proxyTransport := t.Clone()
		proxyTransport.Proxy = http.ProxyURL(&url.URL{
			Scheme: "http",
		})
		// Override the dialer to connect to the Unix socket for the proxy
		proxyTransport.DialContext = func(ctx context.Context, network, addr string) (net.Conn, error) {
			return socket.DialUnix(ctx, desktop.Paths().ProxySocket)
		}

		// Return a fallback transport that tries the proxy first, then falls back to direct
		return newFallbackTransport(proxyTransport, transport)
	}

	return transport
}

// fallbackTransport wraps a proxy transport and falls back to a direct transport
// when the proxy socket becomes unavailable (e.g., Docker Desktop proxy dies).
type fallbackTransport struct {
	proxy  *http.Transport
	direct *http.Transport

	// proxyDisabled is set to true when the proxy socket becomes unavailable.
	// Once set, all subsequent requests go directly without trying the proxy.
	proxyDisabled atomic.Bool
}

// newFallbackTransport creates a transport that tries the proxy first, then falls back to direct.
func newFallbackTransport(proxy, direct *http.Transport) *fallbackTransport {
	return &fallbackTransport{
		proxy:  proxy,
		direct: direct,
	}
}

// DisableCompression disables automatic gzip compression on both transports.
// This is needed for SSE streaming compatibility.
func (f *fallbackTransport) DisableCompression() {
	f.proxy.DisableCompression = true
	f.direct.DisableCompression = true
}

// RoundTrip implements http.RoundTripper.
func (f *fallbackTransport) RoundTrip(req *http.Request) (*http.Response, error) {
	// If proxy is already known to be disabled, go direct
	if f.proxyDisabled.Load() {
		return f.direct.RoundTrip(req)
	}

	// Try the proxy first
	resp, err := f.proxy.RoundTrip(req)
	if err == nil {
		return resp, nil
	}

	// Check if this is a proxy socket error (socket gone, connection refused, etc.)
	if isProxySocketError(err) {
		slog.Warn("Docker Desktop proxy unavailable, falling back to direct connection",
			"error", err.Error(),
			"url", req.URL.String())

		// Disable proxy for future requests
		f.proxyDisabled.Store(true)

		// Clone the request for retry (the body may have been partially read)
		// For requests without a body or with GetBody set, we can retry
		if req.Body == nil || req.GetBody != nil {
			retryReq := req.Clone(req.Context())
			if req.GetBody != nil {
				var bodyErr error
				retryReq.Body, bodyErr = req.GetBody()
				if bodyErr != nil {
					return nil, err // Return original error if we can't get the body
				}
			}
			return f.direct.RoundTrip(retryReq)
		}

		// Can't retry requests with consumed bodies
		return nil, err
	}

	return nil, err
}

// isProxySocketError reports whether err indicates the Docker Desktop proxy
// socket is unavailable: socket file gone, nothing listening on it, or any
// failure dialing a Unix socket. Detection is type-based (net.OpError /
// syscall errno) rather than string matching, so it stays correct across
// Go versions and locales.
func isProxySocketError(err error) bool {
	if err == nil {
		return false
	}

	// Connection-level failures: ENOENT (socket file deleted) or
	// ECONNREFUSED (socket exists but no listener). The HTTP transport
	// wraps these in *net.OpError{Op: "proxyconnect"} → *os.SyscallError,
	// but errors.Is unwraps the chain for us.
	if errors.Is(err, syscall.ENOENT) || errors.Is(err, syscall.ECONNREFUSED) {
		return true
	}

	// Any failure that originated from dialing the proxy: either the
	// HTTP transport's "proxyconnect" wrapper, or a direct Unix-socket
	// dial error from our own DialContext.
	var opErr *net.OpError
	if errors.As(err, &opErr) {
		if opErr.Op == "proxyconnect" {
			return true
		}
		if opErr.Op == "dial" && opErr.Net == "unix" {
			return true
		}
	}

	return false
}
