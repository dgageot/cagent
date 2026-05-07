package dmr

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"net/url"
	"strings"
)

// Unload asks Docker Model Runner to release the resources held for the
// configured model. Invoked by the runtime's `unload` on_agent_switch
// builtin hook.
//
// The unload endpoint is the provider's `unload_api` (relative path or
// absolute URL) when set, otherwise [defaultUnloadURL] derived from the
// OpenAI base URL. When neither is available the call is a no-op.
func (c *Client) Unload(ctx context.Context) error {
	endpoint, err := c.resolveUnloadURL()
	if err != nil || endpoint == "" {
		return err
	}
	return postUnloadModel(ctx, c.httpClient, endpoint, c.ModelConfig.Model)
}

func (c *Client) resolveUnloadURL() (string, error) {
	if override := c.ModelConfig.UnloadAPI(); override != "" {
		return rebaseURL(c.baseURL, override)
	}
	if c.baseURL == "" {
		return "", nil
	}
	return defaultUnloadURL(c.baseURL), nil
}

// defaultUnloadURL derives the `_unload` endpoint URL from the OpenAI
// base URL by replacing the trailing `/v1` segment, mirroring how
// [buildConfigureURL] derives `_configure`:
//
//	http://host:port/engines/v1/             → http://host:port/engines/_unload
//	http://host:port/engines/llama.cpp/v1/   → http://host:port/engines/llama.cpp/_unload
//	http://_/exp/vDD4.40/engines/v1          → http://_/exp/vDD4.40/engines/_unload
func defaultUnloadURL(baseURL string) string {
	u, err := url.Parse(baseURL)
	if err != nil {
		return strings.TrimSuffix(strings.TrimSuffix(baseURL, "/"), "/v1") + "/_unload"
	}
	u.Path = strings.TrimSuffix(strings.TrimSuffix(u.Path, "/"), "/v1") + "/_unload"
	return u.String()
}

// rebaseURL returns path verbatim if it is already an absolute URL,
// otherwise attaches it to baseURL's scheme + host (dropping any path
// baseURL may carry). This lets users point base_url at e.g.
// http://localhost:12434/engines/v1 and override unload_api with
// /engines/_unload without the version prefix bleeding through.
func rebaseURL(baseURL, path string) (string, error) {
	if strings.HasPrefix(path, "http://") || strings.HasPrefix(path, "https://") {
		return path, nil
	}
	u, err := url.Parse(baseURL)
	if err != nil || u.Scheme == "" || u.Host == "" {
		return "", fmt.Errorf("base_url %q is not absolute; cannot resolve %q", baseURL, path)
	}
	if !strings.HasPrefix(path, "/") {
		path = "/" + path
	}
	return u.Scheme + "://" + u.Host + path, nil
}

// postUnloadModel issues `POST <endpoint>` with body `{"model": "<modelID>"}`.
func postUnloadModel(ctx context.Context, client *http.Client, endpoint, modelID string) error {
	body, _ := json.Marshal(map[string]string{"model": modelID})

	req, err := http.NewRequestWithContext(ctx, http.MethodPost, endpoint, bytes.NewReader(body))
	if err != nil {
		return fmt.Errorf("building unload request: %w", err)
	}
	req.Header.Set("Content-Type", "application/json")

	slog.DebugContext(ctx, "Unloading model", "url", endpoint, "model", modelID)

	resp, err := client.Do(req)
	if err != nil {
		return fmt.Errorf("calling unload endpoint %s: %w", endpoint, err)
	}
	defer resp.Body.Close()

	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		respBody, _ := io.ReadAll(io.LimitReader(resp.Body, 4*1024))
		return fmt.Errorf("unload endpoint returned %d: %s",
			resp.StatusCode, strings.TrimSpace(string(respBody)))
	}
	// Drain the success-path body so the underlying transport can reuse
	// the connection (Go's http.Client only re-pools a connection whose
	// body has been read to EOF and closed).
	_, _ = io.Copy(io.Discard, resp.Body)
	return nil
}
