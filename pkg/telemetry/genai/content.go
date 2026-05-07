package genai

import (
	"encoding/json"
	"os"
	"strings"

	"go.opentelemetry.io/otel/attribute"

	"github.com/docker/docker-agent/pkg/chat"
	"github.com/docker/docker-agent/pkg/tools"
)

// EnvCaptureMessageContent is the OTel env var that toggles capture of
// GenAI request/response content as span attributes. Default off (PII).
const EnvCaptureMessageContent = "OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT"

// IsContentCaptureEnabled reports whether the content-capture opt-in is set.
func IsContentCaptureEnabled() bool {
	switch strings.ToLower(strings.TrimSpace(os.Getenv(EnvCaptureMessageContent))) {
	case "true", "1", "yes", "on":
		return true
	default:
		return false
	}
}

// messagePart matches the OTel GenAI semconv message part schema.
type messagePart struct {
	Type      string `json:"type"`
	Content   string `json:"content,omitempty"`
	URI       string `json:"uri,omitempty"`
	MimeType  string `json:"mime_type,omitempty"`
	Modality  string `json:"modality,omitempty"`
	ID        string `json:"id,omitempty"`
	Name      string `json:"name,omitempty"`
	Arguments any    `json:"arguments,omitempty"`
	Result    any    `json:"result,omitempty"`
}

type structuredMessage struct {
	Role  string        `json:"role"`
	Parts []messagePart `json:"parts"`
}

// SetInputMessages serialises chat history into `gen_ai.input.messages`
// (role + parts). System messages are emitted separately as
// `gen_ai.system_instructions`. No-op when content capture is disabled.
func SetInputMessages(span *ChatSpan, messages []chat.Message) {
	if span == nil || !IsContentCaptureEnabled() {
		return
	}

	var systemInstructions []structuredMessage
	var input []structuredMessage
	for i := range messages {
		msg := messageToStructured(&messages[i])
		if messages[i].Role == chat.MessageRoleSystem {
			systemInstructions = append(systemInstructions, msg)
			continue
		}
		input = append(input, msg)
	}

	if len(systemInstructions) > 0 {
		if encoded, err := json.Marshal(systemInstructions); err == nil {
			span.SetAttributes(attribute.String(AttrSystemInstructions, string(encoded)))
		}
	}
	if len(input) > 0 {
		if encoded, err := json.Marshal(input); err == nil {
			span.SetAttributes(attribute.String(AttrInputMessages, string(encoded)))
		}
	}
}

// SetOutputMessages serialises the assembled response into `gen_ai.output.messages`.
func SetOutputMessages(span *ChatSpan, content, reasoning string, toolCalls []tools.ToolCall) {
	if span == nil || !IsContentCaptureEnabled() {
		return
	}
	parts := []messagePart{}
	if reasoning != "" {
		parts = append(parts, messagePart{Type: "reasoning", Content: reasoning})
	}
	if content != "" {
		parts = append(parts, messagePart{Type: "text", Content: content})
	}
	for _, tc := range toolCalls {
		parts = append(parts, messagePart{
			Type:      "tool_call",
			ID:        tc.ID,
			Name:      tc.Function.Name,
			Arguments: tc.Function.Arguments,
		})
	}
	if len(parts) == 0 {
		return
	}
	out := []structuredMessage{{Role: "assistant", Parts: parts}}
	if encoded, err := json.Marshal(out); err == nil {
		span.SetAttributes(attribute.String(AttrOutputMessages, string(encoded)))
	}
}

// SetToolDefinitions serialises the tool definitions presented to the
// model into `gen_ai.tool.definitions`.
func SetToolDefinitions(span *ChatSpan, toolDefs []tools.Tool) {
	if span == nil || !IsContentCaptureEnabled() || len(toolDefs) == 0 {
		return
	}
	encoded, err := json.Marshal(toolDefs)
	if err != nil {
		return
	}
	span.SetAttributes(attribute.String(AttrToolDefinitions, string(encoded)))
}

// messageToStructured converts a chat.Message to the spec-shaped
// structured message representation.
func messageToStructured(m *chat.Message) structuredMessage {
	role := string(m.Role)
	parts := []messagePart{}

	switch {
	case len(m.MultiContent) > 0:
		for _, mc := range m.MultiContent {
			switch mc.Type {
			case chat.MessagePartTypeText:
				if mc.Text != "" {
					parts = append(parts, messagePart{Type: "text", Content: mc.Text})
				}
			case chat.MessagePartTypeImageURL:
				if mc.ImageURL != nil && mc.ImageURL.URL != "" {
					parts = append(parts, messagePart{
						Type:     "uri",
						URI:      mc.ImageURL.URL,
						Modality: "image",
					})
				}
			case chat.MessagePartTypeFile:
				if mc.File != nil {
					p := messagePart{Type: "file", ID: mc.File.FileID}
					if mc.File.MimeType != "" {
						p.MimeType = mc.File.MimeType
					}
					parts = append(parts, p)
				}
			}
		}
	case m.ToolCallID != "":
		// Tool result message: emit only the tool_call_response part below.
	default:
		if m.ReasoningContent != "" {
			parts = append(parts, messagePart{Type: "reasoning", Content: m.ReasoningContent})
		}
		if m.Content != "" {
			parts = append(parts, messagePart{Type: "text", Content: m.Content})
		}
	}

	for _, tc := range m.ToolCalls {
		parts = append(parts, messagePart{
			Type:      "tool_call",
			ID:        tc.ID,
			Name:      tc.Function.Name,
			Arguments: tc.Function.Arguments,
		})
	}
	if m.ToolCallID != "" {
		// tool_call_response carries the payload in `result` per spec.
		parts = append(parts, messagePart{
			Type:   "tool_call_response",
			ID:     m.ToolCallID,
			Result: m.Content,
		})
	}

	return structuredMessage{Role: role, Parts: parts}
}
