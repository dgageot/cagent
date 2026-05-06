package dmr

import (
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"

	"github.com/docker/docker-agent/pkg/config/latest"
	"github.com/docker/docker-agent/pkg/model/provider/base"
)

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
		{
			name:     "container internal host",
			baseURL:  "http://model-runner.docker.internal/engines/v1/",
			expected: "http://model-runner.docker.internal/engines/_unload",
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
func newClient(t *testing.T, baseURL string, httpClient *http.Client, cfg latest.ModelConfig) *Client {
	t.Helper()
	return &Client{
		Config:     base.Config{ModelConfig: cfg},
		baseURL:    baseURL,
		httpClient: httpClient,
	}
}

func TestClientUnloadEndpoint(t *testing.T) {
	t.Parallel()

	tests := []struct {
		name    string
		baseURL string
		cfg     latest.ModelConfig
		want    string
	}{
		{
			name:    "user-configured absolute URL wins",
			baseURL: "http://default.example/engines/v1",
			cfg: latest.ModelConfig{
				ProviderOpts: map[string]any{"unload_api": "https://other.example/api/unload"},
			},
			want: "https://other.example/api/unload",
		},
		{
			name:    "user-configured relative path resolves against base host",
			baseURL: "http://localhost:12434/engines/llama.cpp/v1",
			cfg: latest.ModelConfig{
				ProviderOpts: map[string]any{"unload_api": "/api/unload"},
			},
			want: "http://localhost:12434/api/unload",
		},
		{
			name:    "default endpoint derived from base URL",
			baseURL: "http://localhost:12434/engines/llama.cpp/v1",
			want:    "http://localhost:12434/engines/llama.cpp/_unload",
		},
		{
			name: "empty base URL returns empty endpoint",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			t.Parallel()
			c := newClient(t, tt.baseURL, nil, tt.cfg)
			got, err := c.unloadEndpoint()
			require.NoError(t, err)
			assert.Equal(t, tt.want, got)
		})
	}
}

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

		c := newClient(t, server.URL+"/engines/llama.cpp/v1", server.Client(), latest.ModelConfig{
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

		c := newClient(t, server.URL+"/engines/v1", server.Client(), latest.ModelConfig{
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

		c := newClient(t, server.URL+"/engines/v1", server.Client(), latest.ModelConfig{
			Model: "ai/qwen3",
		})

		err := c.Unload(t.Context())
		require.Error(t, err)
		assert.Contains(t, err.Error(), "500")
		assert.Contains(t, err.Error(), "boom")
	})

	t.Run("no-op when neither base URL nor unload_api are set", func(t *testing.T) {
		t.Parallel()
		c := newClient(t, "", nil, latest.ModelConfig{Model: "ai/qwen3"})
		require.NoError(t, c.Unload(t.Context()))
	})
}
