package tools

import (
	"context"
	"encoding/json"

	"github.com/modelcontextprotocol/go-sdk/mcp"
)

// ToolSet defines the interface for a set of tools.
type ToolSet interface {
	Tools(ctx context.Context) ([]Tool, error)
}

// NewHandler creates a type-safe tool handler from a function that accepts typed parameters.
// It handles JSON unmarshaling of the tool call arguments into the specified type T.
func NewHandler[T any](fn func(context.Context, T) (*ToolCallResult, error)) ToolHandler {
	return func(ctx context.Context, toolCall ToolCall) (*ToolCallResult, error) {
		var params T
		args := toolCall.Function.Arguments
		if args == "" {
			args = "{}"
		}
		if err := json.Unmarshal([]byte(args), &params); err != nil {
			return nil, err
		}
		return fn(ctx, params)
	}
}

type ToolHandler func(ctx context.Context, toolCall ToolCall) (*ToolCallResult, error)

type ToolCall struct {
	ID       string       `json:"id,omitempty"`
	Type     ToolType     `json:"type"`
	Function FunctionCall `json:"function"`
}

type FunctionCall struct {
	Name      string `json:"name,omitempty"`
	Arguments string `json:"arguments,omitempty"`
}

// MediaContent represents base64-encoded binary data (image, audio, etc.)
// returned by a tool.
type MediaContent struct {
	// Data is the base64-encoded payload.
	Data string `json:"data"`
	// MimeType identifies the content type (e.g. "image/png", "audio/wav").
	MimeType string `json:"mimeType"`
}

// ImageContent is an alias kept for readability at call sites.
type ImageContent = MediaContent

// AudioContent is an alias kept for readability at call sites.
type AudioContent = MediaContent

type ToolCallResult struct {
	Output  string `json:"output"`
	IsError bool   `json:"isError,omitempty"`
	Meta    any    `json:"meta,omitempty"`
	// Images contains optional image attachments returned by the tool.
	Images []MediaContent `json:"images,omitempty"`
	// Audios contains optional audio attachments returned by the tool.
	Audios []MediaContent `json:"audios,omitempty"`
	// StructuredContent holds optional structured output returned by an MCP
	// tool whose definition includes an OutputSchema. When non-nil it is the
	// JSON-decoded structured result from the server.
	StructuredContent any `json:"structuredContent,omitempty"`
}

func ResultError(output string) *ToolCallResult {
	return &ToolCallResult{
		Output:  output,
		IsError: true,
	}
}

func ResultSuccess(output string) *ToolCallResult {
	return &ToolCallResult{
		Output:  output,
		IsError: false,
	}
}

type ToolType string

type Tool struct {
	Name                    string          `json:"name"`
	Category                string          `json:"category"`
	Description             string          `json:"description,omitempty"`
	Parameters              any             `json:"parameters"`
	Annotations             ToolAnnotations `json:"annotations"`
	OutputSchema            any             `json:"outputSchema"`
	Handler                 ToolHandler     `json:"-"`
	AddDescriptionParameter bool            `json:"-"`
	// ModelOverride is the per-toolset model for the LLM turn that processes
	// this tool's results. Set automatically from the toolset "model" field.
	ModelOverride string `json:"-"`
}

type ToolAnnotations mcp.ToolAnnotations
