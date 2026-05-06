package openai

import (
	"context"

	"github.com/docker/docker-agent/pkg/httpclient"
	"github.com/docker/docker-agent/pkg/model/provider/base"
)

// Unload asks the upstream provider to release the resources held for the
// configured model. It is a no-op when the provider config does not declare
// an `unload_api`, which is the case for cloud providers (OpenAI, Anthropic
// gateways, ...) that don't expose such an endpoint.
//
// Local OpenAI-compatible inference engines (ollama, ramalama, vLLM, ...)
// typically do, and the runtime triggers Unload when switching away from an
// agent whose model has `provider_opts.unload_on_switch: true`.
func (c *Client) Unload(ctx context.Context) error {
	path := c.ModelConfig.UnloadAPI()
	if path == "" {
		return nil
	}
	endpoint, err := base.JoinHostAndPath(c.ModelConfig.BaseURL, path)
	if err != nil {
		return err
	}
	return base.PostUnloadModel(ctx, httpclient.NewHTTPClient(ctx), endpoint, c.ModelConfig.Model)
}
