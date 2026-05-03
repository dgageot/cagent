package builtin

import (
	"bytes"
	"cmp"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"time"

	"github.com/docker/docker-agent/pkg/config/latest"
	"github.com/docker/docker-agent/pkg/httpclient"
	"github.com/docker/docker-agent/pkg/js"
	"github.com/docker/docker-agent/pkg/remote"
	"github.com/docker/docker-agent/pkg/tools"
)

type APITool struct {
	config   latest.APIToolConfig
	expander *js.Expander
}

// Verify interface compliance
var (
	_ tools.ToolSet      = (*APITool)(nil)
	_ tools.Instructable = (*APITool)(nil)
)

func (t *APITool) callTool(ctx context.Context, toolCall tools.ToolCall) (*tools.ToolCallResult, error) {
	client := &http.Client{
		Timeout:   30 * time.Second,
		Transport: httpclient.WrapWithOTel(remote.NewTransport(ctx)),
	}

	endpoint := t.config.Endpoint
	var reqBody io.Reader = http.NoBody
	switch t.config.Method {
	case http.MethodGet:
		if toolCall.Function.Arguments != "" {
			var params map[string]string
			if err := json.Unmarshal([]byte(toolCall.Function.Arguments), &params); err != nil {
				return nil, fmt.Errorf("invalid arguments: %w", err)
			}

			endpoint = t.expander.Expand(ctx, endpoint, params)
		}
	case http.MethodPost:
		var params map[string]any
		if err := json.Unmarshal([]byte(toolCall.Function.Arguments), &params); err != nil {
			return nil, fmt.Errorf("invalid arguments: %w", err)
		}

		jsonData, err := json.Marshal(params)
		if err != nil {
			return nil, fmt.Errorf("failed to marshal request body: %w", err)
		}

		reqBody = bytes.NewReader(jsonData)
	}

	req, err := http.NewRequestWithContext(ctx, t.config.Method, endpoint, reqBody)
	if err != nil {
		return nil, fmt.Errorf("failed to create request: %w", err)
	}

	setHeaders(req, t.config.Headers)
	if t.config.Method == http.MethodPost {
		req.Header.Set("Content-Type", "application/json")
	}

	resp, err := client.Do(req)
	if err != nil {
		return nil, fmt.Errorf("request failed: %w", err)
	}
	defer resp.Body.Close()

	maxSize := int64(1 << 20)
	body, err := io.ReadAll(io.LimitReader(resp.Body, maxSize))
	if err != nil {
		return nil, fmt.Errorf("failed to read response body: %w", err)
	}

	return tools.ResultSuccess(limitOutput(string(body))), nil
}

func NewAPITool(config latest.APIToolConfig, expander *js.Expander) *APITool {
	return &APITool{
		config:   config,
		expander: expander,
	}
}

func (t *APITool) Instructions() string {
	return t.config.Instruction
}

func (t *APITool) Tools(context.Context) ([]tools.Tool, error) {
	inputSchema, err := tools.SchemaToMap(map[string]any{
		"type":       "object",
		"properties": t.config.Args,
		"required":   t.config.Required,
	})
	if err != nil {
		return nil, fmt.Errorf("invalid schema: %w", err)
	}

	parsedURL, err := url.Parse(t.config.Endpoint)
	if err != nil {
		return nil, fmt.Errorf("invalid URL: %w", err)
	}

	if parsedURL.Scheme == "" || parsedURL.Host == "" {
		return nil, errors.New("invalid URL: missing scheme or host")
	}

	if parsedURL.Scheme != "http" && parsedURL.Scheme != "https" {
		return nil, errors.New("only HTTP and HTTPS URLs are supported")
	}

	outputSchema := tools.MustSchemaFor[string]()
	if t.config.OutputSchema != nil {
		var err error
		outputSchema, err = tools.SchemaToMap(t.config.OutputSchema)
		if err != nil {
			return nil, fmt.Errorf("invalid output_schema: %w", err)
		}
	}

	return []tools.Tool{
		{
			Name:         t.config.Name,
			Category:     "api",
			Description:  t.config.Instruction,
			Parameters:   inputSchema,
			OutputSchema: outputSchema,
			Handler:      t.callTool,
			Annotations: tools.ToolAnnotations{
				ReadOnlyHint: true,
				Title:        cmp.Or(t.config.Name, "Query API"),
			},
		},
	}, nil
}
