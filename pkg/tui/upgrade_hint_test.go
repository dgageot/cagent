package tui

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"gotest.tools/v3/assert"

	"github.com/docker/docker-agent/pkg/paths"
	"github.com/docker/docker-agent/pkg/version/check"
)

// seedVersionCache writes a cache entry for the version checker into a per-test
// cache dir so that the check.LatestCached call inside buildStatusBarTitle
// returns deterministic results without ever hitting the network.
func seedVersionCache(t *testing.T, latest string) {
	t.Helper()

	dir := t.TempDir()
	prev := paths.GetCacheDir()
	paths.SetCacheDir(dir)
	t.Cleanup(func() { paths.SetCacheDir(prev) })

	payload := struct {
		LatestVersion string `json:"latest_version"`
		CheckedAt     int64  `json:"checked_at"`
	}{
		LatestVersion: latest,
		CheckedAt:     time.Now().Unix(),
	}
	data, err := json.Marshal(payload)
	assert.NilError(t, err)

	// Mirror the package's on-disk filename — kept intentionally simple
	// rather than re-exporting the constant from check.
	assert.NilError(t, os.WriteFile(filepath.Join(dir, "version-check.json"), data, 0o600))
}

func TestBuildStatusBarTitle_NoUpgrade(t *testing.T) {
	seedVersionCache(t, "v1.0.0")

	got := buildStatusBarTitle("docker agent", "v1.0.0")
	assert.Equal(t, "docker agent v1.0.0", got)
}

func TestBuildStatusBarTitle_UpgradeAvailable(t *testing.T) {
	seedVersionCache(t, "v1.2.3")

	got := buildStatusBarTitle("docker agent", "v1.0.0")
	assert.Assert(t, strings.Contains(got, "docker agent v1.0.0"))
	assert.Assert(t, strings.Contains(got, "update available: v1.2.3"))
}

func TestBuildStatusBarTitle_DevBuildSilent(t *testing.T) {
	seedVersionCache(t, "v1.2.3")

	got := buildStatusBarTitle("docker agent", "dev")
	assert.Equal(t, "docker agent dev", got)
}

func TestBuildStatusBarTitle_DisabledSilent(t *testing.T) {
	seedVersionCache(t, "v1.2.3")
	t.Setenv(check.DisableEnvVar, "1")

	got := buildStatusBarTitle("docker agent", "v1.0.0")
	assert.Equal(t, "docker agent v1.0.0", got)
}
