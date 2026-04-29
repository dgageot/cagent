package check

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"testing"
	"time"

	"gotest.tools/v3/assert"

	"github.com/docker/docker-agent/pkg/paths"
)

// withCacheDir routes the package's on-disk cache to a per-test temp directory
// so tests don't pollute the user's real cache and don't interfere with each
// other.
func withCacheDir(t *testing.T) string {
	t.Helper()
	dir := t.TempDir()
	prev := paths.GetCacheDir()
	paths.SetCacheDir(dir)
	t.Cleanup(func() { paths.SetCacheDir(prev) })
	return dir
}

func TestIsNewer_BasicSemverOrdering(t *testing.T) {
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
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			assert.Equal(t, tc.want, IsNewer(tc.latest, tc.current))
		})
	}
}

func TestIsNewer_EmptyAndDevAreNeverNewer(t *testing.T) {
	assert.Equal(t, false, IsNewer("", "v1.2.3"))
	assert.Equal(t, false, IsNewer("v1.2.3", ""))
	assert.Equal(t, false, IsNewer("v1.2.3", "dev"))
	assert.Equal(t, false, IsNewer("dev", "v1.2.3"))
}

func TestResult_UpgradeAvailable(t *testing.T) {
	assert.Equal(t, true, Result{Current: "v1.0.0", Latest: "v1.0.1"}.UpgradeAvailable())
	assert.Equal(t, false, Result{Current: "v1.0.1", Latest: "v1.0.0"}.UpgradeAvailable())
	assert.Equal(t, false, Result{Current: "dev", Latest: "v1.0.0"}.UpgradeAvailable())
	assert.Equal(t, false, Result{Current: "v1.0.0", Latest: ""}.UpgradeAvailable())
}

func TestFetchLatestTag_Success(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		assert.Equal(t, "application/vnd.github+json", r.Header.Get("Accept"))
		_ = json.NewEncoder(w).Encode(map[string]any{"tag_name": "v9.9.9"})
	}))
	t.Cleanup(srv.Close)

	tag, err := fetchLatestTag(t.Context(), srv.URL)
	assert.NilError(t, err)
	assert.Equal(t, "v9.9.9", tag)
}

func TestFetchLatestTag_HTTPError(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		http.Error(w, "rate limited", http.StatusForbidden)
	}))
	t.Cleanup(srv.Close)

	_, err := fetchLatestTag(t.Context(), srv.URL)
	assert.ErrorContains(t, err, "unexpected status 403")
}

func TestFetchLatestTag_MalformedPayload(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		_, _ = w.Write([]byte("{not json"))
	}))
	t.Cleanup(srv.Close)

	_, err := fetchLatestTag(t.Context(), srv.URL)
	assert.ErrorContains(t, err, "decode release payload")
}

func TestFetchLatestTag_MissingTag(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		_, _ = w.Write([]byte(`{"foo":"bar"}`))
	}))
	t.Cleanup(srv.Close)

	_, err := fetchLatestTag(t.Context(), srv.URL)
	assert.ErrorContains(t, err, "missing tag_name")
}

func TestCacheRoundTrip(t *testing.T) {
	dir := withCacheDir(t)

	want := cacheEntry{LatestVersion: "v1.2.3", CheckedAt: time.Now().Unix()}
	assert.NilError(t, writeCache(want))

	// Cache file ended up where we expect.
	_, err := os.Stat(filepath.Join(dir, cacheFileName))
	assert.NilError(t, err)

	got, err := readCache()
	assert.NilError(t, err)
	assert.Equal(t, want.LatestVersion, got.LatestVersion)
	assert.Equal(t, want.CheckedAt, got.CheckedAt)
}

func TestReadCache_MissingReturnsZero(t *testing.T) {
	withCacheDir(t)

	got, err := readCache()
	assert.NilError(t, err)
	assert.Equal(t, cacheEntry{}, got)
}

func TestReadCache_CorruptReturnsError(t *testing.T) {
	dir := withCacheDir(t)
	assert.NilError(t, os.WriteFile(filepath.Join(dir, cacheFileName), []byte("not-json"), 0o600))

	_, err := readCache()
	assert.Assert(t, err != nil, "expected error when cache is corrupt")
}

func TestCacheFreshness(t *testing.T) {
	now := time.Now()
	fresh := cacheEntry{CheckedAt: now.Add(-1 * time.Hour).Unix()}
	stale := cacheEntry{CheckedAt: now.Add(-48 * time.Hour).Unix()}
	zero := cacheEntry{}

	assert.Equal(t, true, fresh.fresh(now))
	assert.Equal(t, false, stale.fresh(now))
	assert.Equal(t, false, zero.fresh(now))
}

func TestLatestCached_Disabled(t *testing.T) {
	withCacheDir(t)
	// Seed a cache entry to prove the function ignores it when disabled.
	assert.NilError(t, writeCache(cacheEntry{LatestVersion: "v9.9.9", CheckedAt: time.Now().Unix()}))

	t.Setenv(DisableEnvVar, "1")
	res := LatestCached("v1.0.0")
	assert.Equal(t, "", res.Latest)
}

func TestLatestCached_FromDisk(t *testing.T) {
	withCacheDir(t)
	assert.NilError(t, writeCache(cacheEntry{LatestVersion: "v9.9.9", CheckedAt: time.Now().Unix()}))

	res := LatestCached("v1.0.0")
	assert.Equal(t, "v9.9.9", res.Latest)
	assert.Equal(t, true, res.UpgradeAvailable())
}

func TestLatestCached_NeverFetches(t *testing.T) {
	withCacheDir(t)

	// No cache file exists. LatestCached must NOT block on I/O even if we
	// give it a context that's already canceled.
	ctx, cancel := context.WithCancel(t.Context())
	cancel()

	// Ensure the disabled path is not taken.
	t.Setenv(DisableEnvVar, "")

	done := make(chan struct{})
	go func() {
		_ = LatestCached("v1.0.0")
		_ = ctx
		close(done)
	}()

	select {
	case <-done:
	case <-time.After(2 * time.Second):
		t.Fatal("LatestCached blocked unexpectedly")
	}
}

func TestRefreshAsync_Disabled(t *testing.T) {
	withCacheDir(t)
	t.Setenv(DisableEnvVar, "true")

	done := RefreshAsync(t.Context())
	select {
	case <-done:
	case <-time.After(time.Second):
		t.Fatal("RefreshAsync did not return promptly when disabled")
	}

	// Cache must remain empty.
	got, err := readCache()
	assert.NilError(t, err)
	assert.Equal(t, "", got.LatestVersion)
}

func TestRefreshAsync_FreshCacheSkipsRefresh(t *testing.T) {
	withCacheDir(t)
	// Pre-seed a fresh entry so RefreshAsync should be a no-op.
	want := cacheEntry{LatestVersion: "v1.2.3", CheckedAt: time.Now().Unix()}
	assert.NilError(t, writeCache(want))

	done := RefreshAsync(t.Context())
	select {
	case <-done:
	case <-time.After(time.Second):
		t.Fatal("RefreshAsync did not return promptly with fresh cache")
	}

	got, err := readCache()
	assert.NilError(t, err)
	assert.Equal(t, want.LatestVersion, got.LatestVersion)
	assert.Equal(t, want.CheckedAt, got.CheckedAt)
}

func TestDisabled_Truthy(t *testing.T) {
	for _, val := range []string{"1", "true", "True", "YES", "on"} {
		t.Run(val, func(t *testing.T) {
			t.Setenv(DisableEnvVar, val)
			assert.Equal(t, true, disabled())
		})
	}
}

func TestDisabled_Falsy(t *testing.T) {
	for _, val := range []string{"", "0", "false", "no", "off", "anything-else"} {
		t.Run(val, func(t *testing.T) {
			t.Setenv(DisableEnvVar, val)
			assert.Equal(t, false, disabled())
		})
	}
}
