package openai

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

func TestUnload(t *testing.T) {
	t.Parallel()

	t.Run("no-op when unload_api is not configured", func(t *testing.T) {
		t.Parallel()

		c := &Client{
			Config: base.Config{
				ModelConfig: latest.ModelConfig{
					Provider: "openai",
					Model:    "gpt-4",
					BaseURL:  "https://api.openai.com/v1",
				},
			},
		}
		require.NoError(t, c.Unload(t.Context()))
	})

	t.Run("posts model id to configured unload endpoint", func(t *testing.T) {
		t.Parallel()

		var (
			gotPath string
			gotBody map[string]string
		)
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			gotPath = r.URL.Path
			body, _ := io.ReadAll(r.Body)
			_ = json.Unmarshal(body, &gotBody)
			w.WriteHeader(http.StatusNoContent)
		}))
		defer server.Close()

		c := &Client{
			Config: base.Config{
				ModelConfig: latest.ModelConfig{
					Provider: "ollama",
					Model:    "llama3.2",
					BaseURL:  server.URL + "/v1",
					ProviderOpts: map[string]any{
						"unload_api": "/api/unload",
					},
				},
			},
		}

		require.NoError(t, c.Unload(t.Context()))
		assert.Equal(t, "/api/unload", gotPath)
		assert.Equal(t, map[string]string{"model": "llama3.2"}, gotBody)
	})

	t.Run("returns error on non-2xx", func(t *testing.T) {
		t.Parallel()

		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
			w.WriteHeader(http.StatusBadGateway)
			_, _ = w.Write([]byte("upstream gone"))
		}))
		defer server.Close()

		c := &Client{
			Config: base.Config{
				ModelConfig: latest.ModelConfig{
					Provider: "ollama",
					Model:    "llama3.2",
					BaseURL:  server.URL + "/v1",
					ProviderOpts: map[string]any{
						"unload_api": "/api/unload",
					},
				},
			},
		}

		err := c.Unload(t.Context())
		require.Error(t, err)
		assert.Contains(t, err.Error(), "502")
		assert.Contains(t, err.Error(), "upstream gone")
	})
}
