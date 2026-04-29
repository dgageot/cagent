package check

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"sync"
	"testing"
	"time"

	"gotest.tools/v3/assert"
)

func TestIsNewer(t *testing.T) {
	tests := []struct {
		name    string
		latest  string
		current string
		want    bool
	}{
		{"patch newer", "v1.2.4", "v1.2.3", true},
		{"minor newer", "v1.3.0", "v1.2.9", true},
		{"major newer", "v2.0.0", "v1.99.0", true},
		{"equal", "v1.2.3", "v1.2.3", false},
		{"older", "v1.2.2", "v1.2.3", false},
		{"prefix v optional", "1.2.4", "v1.2.3", true},
		{"missing components", "v1.3", "v1.2.9", true},
		{"release beats prerelease", "v1.2.3", "v1.2.3-rc.1", true},
		{"prerelease loses to release", "v1.2.3-rc.1", "v1.2.3", false},
		{"build metadata ignored", "v1.2.4+abcdef", "v1.2.3", true},
		{"both prerelease equal numeric", "v1.2.3-rc.2", "v1.2.3-rc.1", false},
		{"empty latest", "", "v1.2.3", false},
		{"empty current", "v1.2.3", "", false},
		{"dev current never upgrades", "v1.2.3", "dev", false},
		{"dev latest never upgrades", "dev", "v1.2.3", false},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			assert.Equal(t, tc.want, IsNewer(tc.latest, tc.current))
		})
	}
}

func TestFetchLatestTag(t *testing.T) {
	tests := []struct {
		name       string
		handler    http.HandlerFunc
		wantTag    string
		wantErrSub string
	}{
		{
			name: "success",
			handler: func(w http.ResponseWriter, r *http.Request) {
				assert.Equal(t, "application/vnd.github+json", r.Header.Get("Accept"))
				_ = json.NewEncoder(w).Encode(map[string]any{"tag_name": "v9.9.9"})
			},
			wantTag: "v9.9.9",
		},
		{
			name:       "http error",
			handler:    func(w http.ResponseWriter, _ *http.Request) { http.Error(w, "rate limited", http.StatusForbidden) },
			wantErrSub: "unexpected status 403",
		},
		{
			name:       "malformed payload",
			handler:    func(w http.ResponseWriter, _ *http.Request) { _, _ = w.Write([]byte("{not json")) },
			wantErrSub: "decode release payload",
		},
		{
			name:       "missing tag",
			handler:    func(w http.ResponseWriter, _ *http.Request) { _, _ = w.Write([]byte(`{"foo":"bar"}`)) },
			wantErrSub: "missing tag_name",
		},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			srv := httptest.NewServer(tc.handler)
			t.Cleanup(srv.Close)

			tag, err := fetchLatestTag(t.Context(), srv.URL)
			if tc.wantErrSub != "" {
				assert.ErrorContains(t, err, tc.wantErrSub)
				return
			}
			assert.NilError(t, err)
			assert.Equal(t, tc.wantTag, tag)
		})
	}
}

func TestCacheRoundTrip(t *testing.T) {
	SeedCacheForTest(t, "")

	assert.NilError(t, writeCache("v1.2.3"))

	// Cache file ended up where we expect.
	_, err := os.Stat(cachePath())
	assert.NilError(t, err)

	got, err := readCache()
	assert.NilError(t, err)
	assert.Equal(t, "v1.2.3", got.LatestVersion)
	assert.Assert(t, got.fresh(time.Now()))
}

func TestReadCache_MissingReturnsZero(t *testing.T) {
	SeedCacheForTest(t, "")

	got, err := readCache()
	assert.NilError(t, err)
	assert.Equal(t, cacheEntry{}, got)
}

func TestReadCache_CorruptReturnsZero(t *testing.T) {
	SeedCacheForTest(t, "")
	assert.NilError(t, os.WriteFile(filepath.Join(filepath.Dir(cachePath()), cacheFileName), []byte("not-json"), 0o600))

	got, _ := readCache()
	assert.Equal(t, cacheEntry{}, got)
}

func TestCacheFreshness(t *testing.T) {
	now := time.Now()
	assert.Assert(t, cacheEntry{CheckedAt: now.Add(-1 * time.Hour).Unix()}.fresh(now), "1h-old entry should be fresh")
	assert.Assert(t, !cacheEntry{CheckedAt: now.Add(-48 * time.Hour).Unix()}.fresh(now), "48h-old entry should be stale")
	assert.Assert(t, !cacheEntry{}.fresh(now), "zero entry should be stale")
}

func TestLatestCached(t *testing.T) {
	t.Run("empty cache returns empty", func(t *testing.T) {
		SeedCacheForTest(t, "")
		assert.Equal(t, "", LatestCached("v1.0.0"))
	})

	t.Run("cache newer than current returns latest", func(t *testing.T) {
		SeedCacheForTest(t, "v9.9.9")
		assert.Equal(t, "v9.9.9", LatestCached("v1.0.0"))
	})

	t.Run("cache older than current returns empty", func(t *testing.T) {
		SeedCacheForTest(t, "v1.0.0")
		assert.Equal(t, "", LatestCached("v9.9.9"))
	})

	t.Run("dev current never reports upgrade", func(t *testing.T) {
		SeedCacheForTest(t, "v9.9.9")
		assert.Equal(t, "", LatestCached("dev"))
	})

	t.Run("disabled returns empty even when cache has upgrade", func(t *testing.T) {
		SeedCacheForTest(t, "v9.9.9")
		t.Setenv(DisableEnvVar, "1")
		assert.Equal(t, "", LatestCached("v1.0.0"))
	})
}

func TestRefreshAsync_Disabled(t *testing.T) {
	SeedCacheForTest(t, "")
	t.Setenv(DisableEnvVar, "true")

	<-RefreshAsync(t.Context())

	got, err := readCache()
	assert.NilError(t, err)
	assert.Equal(t, "", got.LatestVersion)
}

func TestRefreshAsync_FreshCacheSkipsRefresh(t *testing.T) {
	SeedCacheForTest(t, "v1.2.3")
	before, err := readCache()
	assert.NilError(t, err)

	<-RefreshAsync(t.Context())

	after, err := readCache()
	assert.NilError(t, err)
	assert.Equal(t, before, after, "fresh cache must not be touched")
}

func TestDisabled(t *testing.T) {
	for _, val := range []string{"1", "true", "True", "YES", "on"} {
		t.Run("truthy/"+val, func(t *testing.T) {
			t.Setenv(DisableEnvVar, val)
			assert.Assert(t, disabled())
		})
	}
	for _, val := range []string{"", "0", "false", "no", "off", "anything-else"} {
		t.Run("falsy/"+val, func(t *testing.T) {
			t.Setenv(DisableEnvVar, val)
			assert.Assert(t, !disabled())
		})
	}
}

func TestFetchLatestTag_RedirectLimit(t *testing.T) {
	redirectCount := 0
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		redirectCount++
		if redirectCount <= 5 {
			http.Redirect(w, r, "/redirect", http.StatusFound)
			return
		}
		_ = json.NewEncoder(w).Encode(map[string]any{"tag_name": "v1.0.0"})
	}))
	t.Cleanup(srv.Close)

	_, err := fetchLatestTag(t.Context(), srv.URL)
	assert.ErrorContains(t, err, "stopped after 3 redirects")
}

func TestRefreshAsync_ConcurrentCalls(t *testing.T) {
	SeedCacheForTest(t, "")

	const numCalls = 10
	var wg sync.WaitGroup
	wg.Add(numCalls)
	
	for i := 0; i < numCalls; i++ {
		go func() {
			defer wg.Done()
			<-RefreshAsync(t.Context())
		}()
	}
	
	wg.Wait()
	// If we get here without panicking or racing, the test passes
}
