package check

import (
	"testing"

	"github.com/docker/docker-agent/pkg/paths"
)

// SeedCacheForTest points the cache directory at a per-test temp dir and
// pre-populates it with the given release tag (or leaves it empty if latest
// is ""). It is intended for unit tests in other packages that want to
// observe [LatestCached] returning a deterministic value without hitting the
// network.
//
// The cache directory override is restored on test cleanup.
func SeedCacheForTest(tb testing.TB, latest string) {
	tb.Helper()

	prev := paths.GetCacheDir()
	paths.SetCacheDir(tb.TempDir())
	tb.Cleanup(func() { paths.SetCacheDir(prev) })

	if latest == "" {
		return
	}
	if err := writeCache(latest); err != nil {
		tb.Fatalf("seed version cache: %v", err)
	}
}
