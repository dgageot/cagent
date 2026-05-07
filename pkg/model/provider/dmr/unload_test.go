package dmr

import (
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"sync"
	"testing"

	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"

	"github.com/docker/docker-agent/pkg/config/latest"
	"github.com/docker/docker-agent/pkg/model/provider/base"
)

func TestRebaseURL(t *testing.T) {
	t.Parallel()

	tests := []struct {
		name        string
		baseURL     string
		path        string
		want        string
		errContains string // empty ⇒ expect success
	}{
		{
			name:    "absolute https URL is returned verbatim",
			baseURL: "http://anything",
			path:    "https://api.example.com/unload",
			want:    "https://api.example.com/unload",
		},
		{
			name:    "rooted path drops base path",
			baseURL: "http://localhost:12434/engines/v1",
			path:    "/engines/_unload",
			want:    "http://localhost:12434/engines/_unload",
		},
		{
			name:    "relative path is rooted",
			baseURL: "http://localhost:12434/engines/v1",
			path:    "engines/_unload",
			want:    "http://localhost:12434/engines/_unload",
		},
		{
			name:        "empty base URL with relative path errors",
			path:        "/engines/_unload",
			errContains: "is not absolute",
		},
		{
			name:        "base URL without scheme errors",
			baseURL:     "localhost:12434/engines/v1",
			path:        "/engines/_unload",
			errContains: "is not absolute",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			t.Parallel()
			got, err := rebaseURL(tt.baseURL, tt.path)
			if tt.errContains != "" {
				require.Error(t, err)
				assert.Contains(t, err.Error(), tt.errContains)
				return
			}
			require.NoError(t, err)
			assert.Equal(t, tt.want, got)
		})
	}
}

func TestDefaultUnloadURL(t *testing.T) {
	t.Parallel()

	tests := []struct {
		name     string
		baseURL  string
		expected string
	}{
		{
			name:     "standard engines path",
			baseURL:  "http://127.0.0.1:12434/engines/v1/",
			expected: "http://127.0.0.1:12434/engines/_unload",
		},
		{
			name:     "no trailing slash",
			baseURL:  "http://127.0.0.1:12434/engines/v1",
			expected: "http://127.0.0.1:12434/engines/_unload",
		},
		{
			name:     "Docker Desktop experimental prefix",
			baseURL:  "http://_/exp/vDD4.40/engines/v1",
			expected: "http://_/exp/vDD4.40/engines/_unload",
		},
		{
			name:     "backend-scoped path",
			baseURL:  "http://127.0.0.1:12434/engines/llama.cpp/v1/",
			expected: "http://127.0.0.1:12434/engines/llama.cpp/_unload",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			t.Parallel()
			assert.Equal(t, tt.expected, defaultUnloadURL(tt.baseURL))
		})
	}
}

// newClient builds a [Client] just well-formed enough to drive Unload.
func newClient(baseURL string, httpClient *http.Client, cfg latest.ModelConfig) *Client {
	return &Client{
		Config:     base.Config{ModelConfig: cfg},
		baseURL:    baseURL,
		httpClient: httpClient,
	}
}

// TestClientUnload exercises the full Unload path end-to-end against an
// httptest server, covering: default URL derivation, user-configured
// unload_api override, non-2xx error surfacing, and the no-op branch
// when no endpoint can be determined.
func TestClientUnload(t *testing.T) {
	t.Parallel()

	t.Run("posts model id to default unload endpoint", func(t *testing.T) {
		t.Parallel()

		var (
			gotPath string
			gotBody map[string]string
		)
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			gotPath = r.URL.Path
			body, _ := io.ReadAll(r.Body)
			_ = json.Unmarshal(body, &gotBody)
			w.WriteHeader(http.StatusOK)
		}))
		defer server.Close()

		c := newClient(server.URL+"/engines/llama.cpp/v1", server.Client(), latest.ModelConfig{
			Provider: "dmr",
			Model:    "ai/qwen3",
		})

		require.NoError(t, c.Unload(t.Context()))
		assert.Equal(t, "/engines/llama.cpp/_unload", gotPath)
		assert.Equal(t, map[string]string{"model": "ai/qwen3"}, gotBody)
	})

	t.Run("honours user-configured unload_api path", func(t *testing.T) {
		t.Parallel()

		var gotPath string
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			gotPath = r.URL.Path
			w.WriteHeader(http.StatusOK)
		}))
		defer server.Close()

		c := newClient(server.URL+"/engines/v1", server.Client(), latest.ModelConfig{
			Model:        "ai/qwen3",
			ProviderOpts: map[string]any{"unload_api": "/custom/unload"},
		})

		require.NoError(t, c.Unload(t.Context()))
		assert.Equal(t, "/custom/unload", gotPath)
	})

	t.Run("returns error on non-2xx", func(t *testing.T) {
		t.Parallel()

		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
			w.WriteHeader(http.StatusInternalServerError)
			_, _ = w.Write([]byte("boom"))
		}))
		defer server.Close()

		c := newClient(server.URL+"/engines/v1", server.Client(), latest.ModelConfig{
			Model: "ai/qwen3",
		})

		err := c.Unload(t.Context())
		require.Error(t, err)
		assert.Contains(t, err.Error(), "500")
		assert.Contains(t, err.Error(), "boom")
	})

	t.Run("no-op when neither base URL nor unload_api are set", func(t *testing.T) {
		t.Parallel()
		c := newClient("", nil, latest.ModelConfig{Model: "ai/qwen3"})
		require.NoError(t, c.Unload(t.Context()))
	})

	t.Run("drains success body so connection can be reused", func(t *testing.T) {
		t.Parallel()

		// httptest's default server uses keep-alive; a non-drained body would
		// force a fresh TCP connection on the second call. We assert that the
		// underlying transport saw a single connection across two POSTs.
		var seen sync.Map
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			seen.Store(r.RemoteAddr, struct{}{})
			// Write a non-trivial body so the drain actually has work to do.
			_, _ = w.Write([]byte(`{"status":"ok"}`))
		}))
		defer server.Close()

		c := newClient(server.URL+"/engines/v1", server.Client(), latest.ModelConfig{Model: "ai/qwen3"})
		require.NoError(t, c.Unload(t.Context()))
		require.NoError(t, c.Unload(t.Context()))

		count := 0
		seen.Range(func(_, _ any) bool { count++; return true })
		assert.Equal(t, 1, count, "both POSTs must reuse the same TCP connection")
	})
}
