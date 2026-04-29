// Package check provides a best-effort upgrade check against GitHub releases.
//
// The check is intentionally lightweight:
//   - The latest release tag is fetched from the GitHub REST API.
//   - The result is cached on disk for 24h to avoid hitting the API on every
//     invocation.
//   - The check is opt-out via the DOCKER_AGENT_DISABLE_VERSION_CHECK=1
//     environment variable.
//   - Failures (offline, rate-limited, parse errors, dev builds, …) are
//     swallowed: the user simply does not see an upgrade hint.
//
// Callers retrieve the latest known version with [Latest]. To refresh the
// cache asynchronously without blocking startup, call [RefreshAsync].
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
	"sync"
	"time"

	"github.com/docker/docker-agent/pkg/paths"
)

// DisableEnvVar is the environment variable that disables the version check
// when set to a truthy value (1, true, yes, on, …). It is honoured by both
// [Latest] and [RefreshAsync].
const DisableEnvVar = "DOCKER_AGENT_DISABLE_VERSION_CHECK"

// CacheTTL is how long a fetched release entry stays valid on disk before a
// refresh is attempted.
const CacheTTL = 24 * time.Hour

// FetchTimeout bounds a single HTTP fetch to the GitHub API.
const FetchTimeout = 5 * time.Second

// defaultReleasesURL is the GitHub REST endpoint used to fetch the latest
// stable release. Pre-release tags are excluded by `/releases/latest`.
const defaultReleasesURL = "https://api.github.com/repos/docker/docker-agent/releases/latest"

// cacheFileName is the on-disk cache filename, stored under [paths.GetCacheDir].
const cacheFileName = "version-check.json"

// cacheEntry is the payload persisted under [paths.GetCacheDir]/[cacheFileName].
type cacheEntry struct {
	// LatestVersion is the latest release tag observed (e.g. "v1.53.0").
	LatestVersion string `json:"latest_version"`
	// CheckedAt is the unix-second timestamp of the last successful fetch.
	CheckedAt int64 `json:"checked_at"`
}

// fresh reports whether the cache entry is still within [CacheTTL].
func (e cacheEntry) fresh(now time.Time) bool {
	if e.CheckedAt <= 0 {
		return false
	}
	return now.Sub(time.Unix(e.CheckedAt, 0)) < CacheTTL
}

// Result describes the outcome of an upgrade check.
type Result struct {
	// Current is the running binary's version string.
	Current string
	// Latest is the latest known stable release tag, or "" if unknown.
	Latest string
}

// UpgradeAvailable returns true when [Result.Latest] is strictly newer than
// [Result.Current]. A "dev" build never reports an upgrade, since its version
// is not comparable to a release tag.
func (r Result) UpgradeAvailable() bool {
	if r.Latest == "" || r.Current == "" || r.Current == "dev" {
		return false
	}
	return IsNewer(r.Latest, r.Current)
}

// Latest returns the most recently cached release, refreshing from the GitHub
// API only when the cache is stale (or missing) and the check is enabled.
//
// If the network call fails for any reason, the stale cache (if any) is
// returned so the caller can still surface a — possibly out of date — hint.
// If there is nothing usable to return, an empty string is returned alongside
// a nil error: the upgrade hint is best-effort and must not surface errors to
// the user.
func Latest(ctx context.Context, current string) Result {
	res := Result{Current: current}

	if disabled() {
		return res
	}

	cache, _ := readCache()

	if cache.fresh(time.Now()) {
		res.Latest = cache.LatestVersion
		return res
	}

	// Cache is stale or missing: try to refresh synchronously, with a tight
	// timeout to avoid blocking the caller.
	fetchCtx, cancel := context.WithTimeout(ctx, FetchTimeout)
	defer cancel()

	tag, err := fetchLatestTag(fetchCtx, defaultReleasesURL)
	if err != nil {
		slog.Debug("Version check fetch failed", "error", err)
		// Fall back to the (stale) cached value — better than nothing.
		res.Latest = cache.LatestVersion
		return res
	}

	if err := writeCache(cacheEntry{LatestVersion: tag, CheckedAt: time.Now().Unix()}); err != nil {
		slog.Debug("Version check cache write failed", "error", err)
	}

	res.Latest = tag
	return res
}

// LatestCached returns the most recently cached release without ever issuing
// a network call. It is intended for callers (such as the TUI) that must not
// block on I/O at startup; combine with [RefreshAsync] to keep the cache warm.
func LatestCached(current string) Result {
	res := Result{Current: current}
	if disabled() {
		return res
	}
	cache, _ := readCache()
	res.Latest = cache.LatestVersion
	return res
}

// RefreshAsync triggers a background refresh of the version check cache when
// it is stale, returning immediately. Errors are logged at debug level and
// otherwise ignored.
//
// The returned channel is closed once the goroutine completes; tests use it
// to deterministically wait for completion. Production callers can ignore it.
func RefreshAsync(ctx context.Context) <-chan struct{} {
	done := make(chan struct{})

	if disabled() {
		close(done)
		return done
	}

	cache, _ := readCache()
	if cache.fresh(time.Now()) {
		close(done)
		return done
	}

	go func() {
		defer close(done)

		fetchCtx, cancel := context.WithTimeout(ctx, FetchTimeout)
		defer cancel()

		tag, err := fetchLatestTag(fetchCtx, defaultReleasesURL)
		if err != nil {
			slog.Debug("Async version check fetch failed", "error", err)
			return
		}
		if err := writeCache(cacheEntry{LatestVersion: tag, CheckedAt: time.Now().Unix()}); err != nil {
			slog.Debug("Async version check cache write failed", "error", err)
		}
	}()

	return done
}

// disabled reports whether the version check has been turned off via the
// [DisableEnvVar] environment variable.
func disabled() bool {
	v := strings.ToLower(strings.TrimSpace(os.Getenv(DisableEnvVar)))
	switch v {
	case "1", "true", "yes", "on":
		return true
	}
	return false
}

// fetchLatestTag returns the `tag_name` field of the latest stable release.
//
// The endpoint is parameterised to keep the function unit-testable.
func fetchLatestTag(ctx context.Context, url string) (string, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, http.NoBody)
	if err != nil {
		return "", err
	}
	// GitHub recommends an explicit Accept header for the v3 REST API.
	req.Header.Set("Accept", "application/vnd.github+json")
	req.Header.Set("X-GitHub-Api-Version", "2022-11-28")

	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		// Drain a little of the body for diagnostics but cap it so a
		// misbehaving server can't blow up memory.
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

// cacheMu guards reads and writes to the on-disk cache file. The cache is
// tiny so we serialise access rather than relying on filesystem atomicity.
var cacheMu sync.Mutex

// cachePath returns the absolute path of the cache file.
func cachePath() string {
	return filepath.Join(paths.GetCacheDir(), cacheFileName)
}

// readCache returns the cached entry, or a zero entry if the file is missing
// or unreadable. Callers should treat a zero entry as "no cache".
func readCache() (cacheEntry, error) {
	cacheMu.Lock()
	defer cacheMu.Unlock()

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

// writeCache persists the given entry to disk, creating the cache directory
// if necessary.
func writeCache(entry cacheEntry) error {
	cacheMu.Lock()
	defer cacheMu.Unlock()

	dir := paths.GetCacheDir()
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return fmt.Errorf("create cache dir: %w", err)
	}

	data, err := json.Marshal(entry)
	if err != nil {
		return err
	}

	// Write+rename for atomicity so a partial write never produces a
	// truncated JSON file that future reads would treat as corrupt.
	tmp, err := os.CreateTemp(dir, cacheFileName+".*")
	if err != nil {
		return err
	}
	tmpName := tmp.Name()
	if _, err := tmp.Write(data); err != nil {
		tmp.Close()
		os.Remove(tmpName)
		return err
	}
	if err := tmp.Close(); err != nil {
		os.Remove(tmpName)
		return err
	}
	return os.Rename(tmpName, cachePath())
}

// IsNewer reports whether the semver-like tag `latest` is strictly greater
// than `current`. The comparison is intentionally tolerant:
//
//   - A leading "v" is stripped from both sides.
//   - Pre-release suffixes (e.g. "-rc.1") are ignored for the numeric
//     comparison and treated as older than their release counterpart.
//   - Components that fail to parse as integers are treated as zero, so
//     malformed inputs simply don't trigger a notification.
//   - When either input is empty or equal to "dev", the function returns
//     false: development builds never get an upgrade prompt.
func IsNewer(latest, current string) bool {
	if latest == "" || current == "" || current == "dev" || latest == "dev" {
		return false
	}

	la, lpre := splitPreRelease(strings.TrimPrefix(latest, "v"))
	cu, cpre := splitPreRelease(strings.TrimPrefix(current, "v"))

	if cmp := compareNumericParts(la, cu); cmp != 0 {
		return cmp > 0
	}

	// Numeric parts are equal: a release (no pre-release) outranks a
	// pre-release of the same version (1.2.3 > 1.2.3-rc.1).
	switch {
	case lpre == "" && cpre != "":
		return true
	case lpre != "" && cpre == "":
		return false
	default:
		// Both have (or don't have) a pre-release: avoid lexical surprises
		// and consider them equal — no upgrade prompt.
		return false
	}
}

// splitPreRelease separates the numeric part of a semver-like string from
// any "-pre" suffix. For example "1.2.3-rc.1" → ("1.2.3", "rc.1").
func splitPreRelease(v string) (string, string) {
	// Drop build metadata first ("+meta"), it never affects ordering.
	if i := strings.Index(v, "+"); i >= 0 {
		v = v[:i]
	}
	if numeric, pre, ok := strings.Cut(v, "-"); ok {
		return numeric, pre
	}
	return v, ""
}

// compareNumericParts compares dotted numeric strings ("1.2.3") component by
// component, returning -1, 0 or +1. Missing trailing components are treated
// as zero so "1.2" == "1.2.0".
func compareNumericParts(a, b string) int {
	ap := strings.Split(a, ".")
	bp := strings.Split(b, ".")
	n := max(len(ap), len(bp))
	for i := range n {
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
