// Package recording provides helpers for recording and replaying AI API interactions.
package recording

import (
	"context"
	"fmt"
	"log/slog"
	"strings"
	"time"

	"github.com/docker/docker-agent/pkg/fake"
)

// SetupFakeProxy starts a fake proxy if fakeResponses is non-empty.
// streamDelayMs controls simulated streaming: 0 = disabled, >0 = delay in milliseconds between chunks.
// It returns the proxy URL and a cleanup function that must be called when done (typically via defer).
func SetupFakeProxy(ctx context.Context, fakeResponses string, streamDelayMs int) (proxyURL string, cleanup func() error, err error) {
	if fakeResponses == "" {
		return "", noop, nil
	}

	// Normalize path by stripping .yaml suffix (go-vcr adds it automatically)
	fakeResponses = strings.TrimSuffix(fakeResponses, ".yaml")

	var opts []fake.ProxyOption
	if streamDelayMs > 0 {
		opts = append(opts,
			fake.WithSimulateStream(true),
			fake.WithStreamChunkDelay(time.Duration(streamDelayMs)*time.Millisecond),
		)
	}

	proxyURL, cleanupFn, err := fake.StartProxy(ctx, fakeResponses, opts...)
	if err != nil {
		return "", nil, fmt.Errorf("failed to start fake proxy: %w", err)
	}

	slog.InfoContext(ctx, "Fake mode enabled", "cassette", fakeResponses, "proxy", proxyURL)

	return proxyURL, cleanupFn, nil
}

// SetupRecordingProxy starts a recording proxy if recordPath is non-empty.
// It handles auto-generating a filename when recordPath is "true" (from NoOptDefVal),
// and normalizes the path by stripping any .yaml suffix.
// Returns the cassette path (with .yaml extension), the proxy URL, and a cleanup function.
// The cleanup function must be called when done (typically via defer).
func SetupRecordingProxy(ctx context.Context, recordPath string) (cassettePath, proxyURL string, cleanup func() error, err error) {
	if recordPath == "" {
		return "", "", noop, nil
	}

	// Handle auto-generated filename (from NoOptDefVal)
	if recordPath == "true" {
		recordPath = fmt.Sprintf("cagent-recording-%d", time.Now().Unix())
	} else {
		recordPath = strings.TrimSuffix(recordPath, ".yaml")
	}

	proxyURL, cleanupFn, err := fake.StartRecordingProxy(ctx, recordPath)
	if err != nil {
		return "", "", nil, fmt.Errorf("failed to start recording proxy: %w", err)
	}

	cassettePath = recordPath + ".yaml"

	slog.InfoContext(ctx, "Recording mode enabled", "cassette", cassettePath, "proxy", proxyURL)

	return cassettePath, proxyURL, cleanupFn, nil
}

func noop() error { return nil }
