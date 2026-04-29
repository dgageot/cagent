// Package check provides a best-effort upgrade hint built from the GitHub
// releases of docker/docker-agent.
//
// Behaviour:
//   - The latest release tag is fetched in the background by [RefreshAsync],
//     called once per `docker agent run` invocation.
//   - The result is cached on disk for 24h so subsequent reads are instant.
//   - [LatestCached] never touches the network: it only consults that cache,
//     so callers (TUI status bar, `version` subcommand) can surface a hint
//     without blocking on I/O.
//   - The whole feature is opt-out via [DisableEnvVar].
//   - All errors (offline, rate-limited, parse errors, dev builds, …) are
//     swallowed: the user simply does not see an upgrade hint.
package check

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"github.com/docker/docker-agent/pkg/paths"
)

// DisableEnvVar is the environment variable that disables the version check
// when set to a truthy value (1, true, yes, on, …).
const DisableEnvVar = "DOCKER_AGENT_DISABLE_VERSION_CHECK"

const (
	cacheTTL      = 24 * time.Hour
	fetchTimeout  = 5 * time.Second
	releasesURL   = "https://api.github.com/repos/docker/docker-agent/releases/latest"
	cacheFileName = "version-check.json"
)

// LatestCached returns the latest known release tag if it is strictly newer
// than current, or "" otherwise.
//
// The function never reaches out to the network — it only consults the local
// cache populated by [RefreshAsync]. It also returns "" when the check is
// disabled or when current is "dev" (development build).
func LatestCached(current string) string {
	if disabled() || current == "" || current == "dev" {
		return ""
	}
	entry, _ := readCache()
	if !IsNewer(entry.LatestVersion, current) {
		return ""
	}
	return entry.LatestVersion
}

// RefreshAsync triggers a background refresh of the on-disk cache when it is
// stale, returning immediately. Errors are logged at debug level and
// otherwise ignored.
//
// The returned channel is closed once the goroutine completes. Tests use it
// to deterministically wait for completion; production callers can ignore it.
func RefreshAsync(ctx context.Context) <-chan struct{} {
	done := make(chan struct{})

	if disabled() {
		close(done)
		return done
	}
	if entry, _ := readCache(); entry.fresh(time.Now()) {
		close(done)
		return done
	}

	go func() {
		defer close(done)

		fetchCtx, cancel := context.WithTimeout(ctx, fetchTimeout)
		defer cancel()

		tag, err := fetchLatestTag(fetchCtx, releasesURL)
		if err != nil {
			slog.Debug("Version check fetch failed", "error", err)
			return
		}
		if err := writeCache(tag); err != nil {
			slog.Debug("Version check cache write failed", "error", err)
		}
	}()

	return done
}

// disabled reports whether the version check has been turned off via
// [DisableEnvVar].
func disabled() bool {
	switch strings.ToLower(strings.TrimSpace(os.Getenv(DisableEnvVar))) {
	case "1", "true", "yes", "on":
		return true
	}
	return false
}

// fetchLatestTag returns the `tag_name` field of the latest stable release.
// The endpoint is parameterised to keep the function unit-testable.
func fetchLatestTag(ctx context.Context, url string) (string, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, http.NoBody)
	if err != nil {
		return "", err
	}
	req.Header.Set("Accept", "application/vnd.github+json")
	req.Header.Set("X-GitHub-Api-Version", "2022-11-28")

	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(io.LimitReader(resp.Body, 512))
		return "", fmt.Errorf("unexpected status %d: %s", resp.StatusCode, strings.TrimSpace(string(body)))
	}

	var payload struct {
		TagName string `json:"tag_name"`
	}
	if err := json.NewDecoder(io.LimitReader(resp.Body, 1<<20)).Decode(&payload); err != nil {
		return "", fmt.Errorf("decode release payload: %w", err)
	}
	if payload.TagName == "" {
		return "", errors.New("release payload missing tag_name")
	}
	return payload.TagName, nil
}

// cacheEntry is the JSON payload persisted to disk.
type cacheEntry struct {
	LatestVersion string `json:"latest_version"`
	CheckedAt     int64  `json:"checked_at"`
}

// fresh reports whether the entry is still within [cacheTTL].
func (e cacheEntry) fresh(now time.Time) bool {
	return e.CheckedAt > 0 && now.Sub(time.Unix(e.CheckedAt, 0)) < cacheTTL
}

// cachePath returns the absolute path of the cache file.
func cachePath() string {
	return filepath.Join(paths.GetCacheDir(), cacheFileName)
}

// readCache returns the cached entry, or a zero entry if the file is missing
// or unreadable. The file is small enough that we do not worry about partial
// reads: if Unmarshal fails, callers simply see "no cache" for one call.
func readCache() (cacheEntry, error) {
	data, err := os.ReadFile(cachePath())
	if err != nil {
		if errors.Is(err, os.ErrNotExist) {
			return cacheEntry{}, nil
		}
		return cacheEntry{}, err
	}
	var entry cacheEntry
	if err := json.Unmarshal(data, &entry); err != nil {
		return cacheEntry{}, err
	}
	return entry, nil
}

// writeCache persists the given release tag along with the current timestamp.
func writeCache(latest string) error {
	if err := os.MkdirAll(paths.GetCacheDir(), 0o755); err != nil {
		return fmt.Errorf("create cache dir: %w", err)
	}
	data, err := json.Marshal(cacheEntry{LatestVersion: latest, CheckedAt: time.Now().Unix()})
	if err != nil {
		return err
	}
	return os.WriteFile(cachePath(), data, 0o600)
}

// IsNewer reports whether the semver-like tag latest is strictly greater than
// current. The comparison is intentionally tolerant:
//
//   - A leading "v" is stripped from both sides.
//   - Build metadata ("+meta") is ignored.
//   - A pre-release ("-rc.1") sorts strictly older than the same release.
//   - Components that fail to parse as integers are treated as 0, so
//     malformed inputs simply do not trigger a notification.
//   - Empty strings or "dev" never compare as newer.
func IsNewer(latest, current string) bool {
	if latest == "" || current == "" || current == "dev" || latest == "dev" {
		return false
	}

	la, lpre := splitVersion(latest)
	cu, cpre := splitVersion(current)

	if cmp := compareNumeric(la, cu); cmp != 0 {
		return cmp > 0
	}
	// Equal numeric parts: a release outranks a pre-release of the same
	// version (1.2.3 > 1.2.3-rc.1). Otherwise treat as equal.
	return lpre == "" && cpre != ""
}

// splitVersion strips a leading "v", drops "+build" metadata, and splits off
// any "-prerelease" suffix. For example "v1.2.3-rc.1+meta" → ("1.2.3", "rc.1").
func splitVersion(v string) (numeric, pre string) {
	v = strings.TrimPrefix(v, "v")
	if i := strings.Index(v, "+"); i >= 0 {
		v = v[:i]
	}
	if num, p, ok := strings.Cut(v, "-"); ok {
		return num, p
	}
	return v, ""
}

// compareNumeric compares dotted numeric strings ("1.2.3") component by
// component, returning -1, 0 or +1. Missing trailing components are treated
// as zero so "1.2" == "1.2.0".
func compareNumeric(a, b string) int {
	ap := strings.Split(a, ".")
	bp := strings.Split(b, ".")
	for i := range max(len(ap), len(bp)) {
		var ai, bi int
		if i < len(ap) {
			ai, _ = strconv.Atoi(ap[i])
		}
		if i < len(bp) {
			bi, _ = strconv.Atoi(bp[i])
		}
		if ai != bi {
			if ai > bi {
				return 1
			}
			return -1
		}
	}
	return 0
}
