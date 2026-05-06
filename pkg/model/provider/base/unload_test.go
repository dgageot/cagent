package base

import (
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

func TestJoinHostAndPath(t *testing.T) {
	t.Parallel()

	tests := []struct {
		name        string
		baseURL     string
		path        string
		want        string
		wantErr     bool
		errContains string
	}{
		{
			name:    "absolute https URL",
			baseURL: "http://anything",
			path:    "https://api.example.com/unload",
			want:    "https://api.example.com/unload",
		},
		{
			name: "absolute http URL",
			path: "http://api.example.com/unload",
			want: "http://api.example.com/unload",
		},
		{
			name:    "rooted path drops base path",
			baseURL: "http://localhost:11434/v1",
			path:    "/api/unload",
			want:    "http://localhost:11434/api/unload",
		},
		{
			name:    "engines prefix on base, /engines/_unload path",
			baseURL: "http://model-runner.docker.internal/engines/llama.cpp/v1",
			path:    "/engines/_unload",
			want:    "http://model-runner.docker.internal/engines/_unload",
		},
		{
			name:    "relative path is rooted",
			baseURL: "http://localhost:11434/v1",
			path:    "api/unload",
			want:    "http://localhost:11434/api/unload",
		},
		{
			name:        "empty path",
			baseURL:     "http://localhost:11434",
			wantErr:     true,
			errContains: "path is empty",
		},
		{
			name:        "whitespace only path",
			baseURL:     "http://localhost:11434",
			path:        "   ",
			wantErr:     true,
			errContains: "path is empty",
		},
		{
			name:        "empty base URL with relative path",
			path:        "/api/unload",
			wantErr:     true,
			errContains: "base_url is empty",
		},
		{
			name:        "base URL without scheme",
			baseURL:     "localhost:11434/v1",
			path:        "/api/unload",
			wantErr:     true,
			errContains: "must include a scheme and host",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			t.Parallel()
			got, err := JoinHostAndPath(tt.baseURL, tt.path)
			if tt.wantErr {
				require.Error(t, err)
				if tt.errContains != "" {
					assert.Contains(t, err.Error(), tt.errContains)
				}
				return
			}
			require.NoError(t, err)
			assert.Equal(t, tt.want, got)
		})
	}
}

func TestPostUnloadModel(t *testing.T) {
	t.Parallel()

	t.Run("posts JSON body and returns nil on 2xx", func(t *testing.T) {
		t.Parallel()

		var (
			gotMethod string
			gotPath   string
			gotCT     string
			gotBody   map[string]string
		)
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			gotMethod = r.Method
			gotPath = r.URL.Path
			gotCT = r.Header.Get("Content-Type")
			body, _ := io.ReadAll(r.Body)
			_ = json.Unmarshal(body, &gotBody)
			w.WriteHeader(http.StatusAccepted)
		}))
		defer server.Close()

		err := PostUnloadModel(t.Context(), server.Client(), server.URL+"/api/unload", "ai/qwen3")
		require.NoError(t, err)

		assert.Equal(t, http.MethodPost, gotMethod)
		assert.Equal(t, "/api/unload", gotPath)
		assert.Equal(t, "application/json", gotCT)
		assert.Equal(t, map[string]string{"model": "ai/qwen3"}, gotBody)
	})

	t.Run("returns error on non-2xx", func(t *testing.T) {
		t.Parallel()

		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
			w.WriteHeader(http.StatusInternalServerError)
			_, _ = w.Write([]byte("model busy"))
		}))
		defer server.Close()

		err := PostUnloadModel(t.Context(), server.Client(), server.URL+"/api/unload", "ai/qwen3")
		require.Error(t, err)
		assert.Contains(t, err.Error(), "500")
		assert.Contains(t, err.Error(), "model busy")
	})

	t.Run("nil http client falls back to default", func(t *testing.T) {
		t.Parallel()

		called := make(chan struct{}, 1)
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
			called <- struct{}{}
			w.WriteHeader(http.StatusNoContent)
		}))
		defer server.Close()

		err := PostUnloadModel(t.Context(), nil, server.URL+"/api/unload", "ai/qwen3")
		require.NoError(t, err)
		select {
		case <-called:
		default:
			t.Fatal("server was not called")
		}
	})
}
