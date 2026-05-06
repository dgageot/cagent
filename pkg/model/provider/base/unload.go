package base

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"net/url"
	"strings"
)

// PostUnloadModel issues `POST <endpoint>` with body `{"model": "<modelID>"}`
// and reads the response so the connection can be reused.
//
// It is shared by [provider.Unloader] implementations that talk to OpenAI-
// compatible engines (DMR, ollama, ramalama, ...). The endpoint is built
// by the caller; see [JoinHostAndPath] for the standard "scheme/host of
// base_url + provider-supplied path" join.
func PostUnloadModel(ctx context.Context, client *http.Client, endpoint, modelID string) error {
	if client == nil {
		client = http.DefaultClient
	}
	body, err := json.Marshal(map[string]string{"model": modelID})
	if err != nil {
		return fmt.Errorf("encoding unload request: %w", err)
	}

	req, err := http.NewRequestWithContext(ctx, http.MethodPost, endpoint, bytes.NewReader(body))
	if err != nil {
		return fmt.Errorf("building unload request: %w", err)
	}
	req.Header.Set("Content-Type", "application/json")

	slog.Debug("Unloading model", "url", endpoint, "model", modelID)

	resp, err := client.Do(req)
	if err != nil {
		return fmt.Errorf("calling unload endpoint %s: %w", endpoint, err)
	}
	defer resp.Body.Close()

	respBody, _ := io.ReadAll(io.LimitReader(resp.Body, 4*1024))
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		return fmt.Errorf("unload endpoint returned %d: %s",
			resp.StatusCode, strings.TrimSpace(string(respBody)))
	}
	return nil
}

// JoinHostAndPath joins baseURL's scheme + host with path, dropping any
// path component baseURL may carry.
//
//   - If path is itself an absolute URL (http:// or https://), it is
//     returned as-is.
//   - Otherwise scheme://host from baseURL is prepended to path. This
//     lets users point base_url at e.g. http://localhost:11434/v1 and
//     configure unload_api: /api/unload without the version prefix
//     bleeding through.
func JoinHostAndPath(baseURL, path string) (string, error) {
	path = strings.TrimSpace(path)
	if path == "" {
		return "", errors.New("path is empty")
	}
	if strings.HasPrefix(path, "http://") || strings.HasPrefix(path, "https://") {
		return path, nil
	}
	if baseURL == "" {
		return "", fmt.Errorf("base_url is empty; cannot resolve relative path %q", path)
	}
	u, err := url.Parse(baseURL)
	if err != nil {
		return "", fmt.Errorf("parsing base_url %q: %w", baseURL, err)
	}
	if u.Scheme == "" || u.Host == "" {
		return "", fmt.Errorf("base_url %q must include a scheme and host", baseURL)
	}
	if !strings.HasPrefix(path, "/") {
		path = "/" + path
	}
	return u.Scheme + "://" + u.Host + path, nil
}
