package tui

import (
	"github.com/docker/docker-agent/pkg/version/check"
)

// buildStatusBarTitle returns the right-side string of the status bar:
// "<appName> <appVersion>", optionally suffixed with "(update available: vX.Y.Z)"
// when a newer release tag has been observed in the local cache.
//
// Only cached results are consulted so the TUI never blocks on I/O at
// startup; the cache is refreshed in the background by `docker agent run`
// (see cmd/root/run.go).
func buildStatusBarTitle(appName, appVersion string) string {
	base := appName + " " + appVersion
	if latest := check.LatestCached(appVersion); latest != "" {
		return base + " (update available: " + latest + ")"
	}
	return base
}
