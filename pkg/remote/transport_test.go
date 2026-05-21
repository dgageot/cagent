package remote

import (
	"errors"
	"net"
	"net/http"
	"net/http/httptest"
	"net/url"
	"os"
	"syscall"
	"testing"

	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"

	"github.com/docker/docker-agent/pkg/desktop"
)

func TestNewTransport_UsesDesktopProxyWhenAvailable(t *testing.T) {
	t.Parallel()

	ctx := t.Context()

	// Create a transport
	transport := NewTransport(ctx)
	require.NotNil(t, transport)

	// If Docker Desktop is running, verify fallback transport is used
	if desktop.IsDockerDesktopRunning(ctx) {
		_, ok := transport.(*fallbackTransport)
		assert.True(t, ok, "transport should be *fallbackTransport when Docker Desktop is running")
	} else {
		// Otherwise, it should be a plain *http.Transport
		_, ok := transport.(*http.Transport)
		assert.True(t, ok, "transport should be *http.Transport when Docker Desktop is not running")
	}
}

func TestNewTransport_WorksWithoutDesktopProxy(t *testing.T) {
	t.Parallel()

	// Create a test server to simulate a registry
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusOK)
	}))
	defer server.Close()

	ctx := t.Context()

	// Create a transport (should work whether Desktop is running or not)
	transport := NewTransport(ctx)
	require.NotNil(t, transport)

	// Make a simple HTTP request to verify the transport works
	client := &http.Client{Transport: transport}
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, server.URL, http.NoBody)
	require.NoError(t, err)
	resp, err := client.Do(req)
	require.NoError(t, err)
	defer resp.Body.Close()

	assert.Equal(t, http.StatusOK, resp.StatusCode)
}

func TestIsProxySocketError(t *testing.T) {
	t.Parallel()

	dialUnixENOENT := &net.OpError{
		Op:  "dial",
		Net: "unix",
		Err: &os.SyscallError{Syscall: "connect", Err: syscall.ENOENT},
	}
	proxyConnectRefused := &net.OpError{
		Op:  "proxyconnect",
		Net: "tcp",
		Err: &os.SyscallError{Syscall: "connect", Err: syscall.ECONNREFUSED},
	}
	proxyConnectGeneric := &net.OpError{
		Op:  "proxyconnect",
		Net: "tcp",
		Err: errors.New("some unrelated error"),
	}
	urlErr := &url.Error{Op: "Get", URL: "https://example/", Err: proxyConnectRefused}

	tests := []struct {
		name     string
		err      error
		expected bool
	}{
		{
			name:     "unix socket missing (ENOENT)",
			err:      dialUnixENOENT,
			expected: true,
		},
		{
			name:     "proxy connect refused",
			err:      proxyConnectRefused,
			expected: true,
		},
		{
			name:     "proxyconnect with unrelated wrapped error",
			err:      proxyConnectGeneric,
			expected: true,
		},
		{
			name:     "wrapped in url.Error (real http.Client failure shape)",
			err:      urlErr,
			expected: true,
		},
		{
			name:     "plain TCP timeout is not a proxy error",
			err:      &net.OpError{Op: "dial", Net: "tcp", Err: errors.New("i/o timeout")},
			expected: false,
		},
		{
			name:     "unrelated error",
			err:      errors.New("HTTP 500"),
			expected: false,
		},
		{
			name:     "nil error",
			err:      nil,
			expected: false,
		},
	}

	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			t.Parallel()
			assert.Equal(t, tc.expected, isProxySocketError(tc.err))
		})
	}
}

func TestFallbackTransport_DisableCompression(t *testing.T) {
	t.Parallel()

	proxy := &http.Transport{}
	direct := &http.Transport{}

	ft := newFallbackTransport(proxy, direct)

	// Verify compression is not disabled initially
	assert.False(t, proxy.DisableCompression)
	assert.False(t, direct.DisableCompression)

	// Disable compression
	ft.DisableCompression()

	// Verify compression is now disabled on both transports
	assert.True(t, proxy.DisableCompression)
	assert.True(t, direct.DisableCompression)
}
