package dmr

import (
	"context"
	"net/url"
	"strings"

	"github.com/docker/docker-agent/pkg/model/provider/base"
)

// Unload asks Docker Model Runner to release the resources held for the
// configured model. It is invoked by the runtime when switching away from
// an agent whose model has `provider_opts.unload_on_switch: true`.
//
// User-configured `unload_api` on the provider wins; otherwise the default
// `_unload` endpoint is derived from the OpenAI base URL by replacing the
// `/v1` suffix, mirroring how [buildConfigureURL] derives `_configure`.
func (c *Client) Unload(ctx context.Context) error {
	endpoint, err := c.unloadEndpoint()
	if err != nil || endpoint == "" {
		return err
	}
	return base.PostUnloadModel(ctx, c.httpClient, endpoint, c.ModelConfig.Model)
}

// unloadEndpoint returns the URL to POST to in order to unload the
// configured model, or "" when no endpoint can be determined.
func (c *Client) unloadEndpoint() (string, error) {
	if path := c.ModelConfig.UnloadAPI(); path != "" {
		return base.JoinHostAndPath(c.baseURL, path)
	}
	if c.baseURL == "" {
		return "", nil
	}
	return defaultUnloadURL(c.baseURL), nil
}

// defaultUnloadURL derives the `_unload` endpoint URL from the OpenAI base
// URL by replacing the trailing `/v1` segment, mirroring [buildConfigureURL]:
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
