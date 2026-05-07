package mcp

import (
	"context"

	"go.opentelemetry.io/otel/baggage"
)

// ConversationIDFromBaggage reads `gen_ai.conversation.id` from the W3C
// baggage in ctx. Mirrors the genai package convention so MCP spans pick
// up the session id automatically.
func ConversationIDFromBaggage(ctx context.Context) string {
	return baggage.FromContext(ctx).Member("gen_ai.conversation.id").Value()
}
