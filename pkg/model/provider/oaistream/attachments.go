package oaistream

import (
	"context"
	"encoding/base64"
	"fmt"
	"log/slog"
	"strings"

	"github.com/openai/openai-go/v3"

	"github.com/docker/docker-agent/pkg/attachment"
	"github.com/docker/docker-agent/pkg/attachment/modelcaps"
	"github.com/docker/docker-agent/pkg/chat"
)

// convertDocument converts a chat.Document to zero or more
// ChatCompletionContentPartUnionParam values using the OpenAI Chat Completions
// format.  It is also used by all oaistream-based providers (Mistral, xAI,
// Ollama, Nebius, MiniMax, GitHub Copilot, Azure, Requesty).
//
// Routing:
//   - image/* with InlineData → data-URI image part
//   - other binary MIMEs with InlineData → text part with TXTEnvelope fallback
//   - text MIMEs with InlineText → text part with TXTEnvelope
//   - unsupported / no content → nil (logged as warning)
func convertDocument(ctx context.Context, doc chat.Document, modelID string) ([]openai.ChatCompletionContentPartUnionParam, error) {
	mc, _ := modelcaps.Load(modelID)
	return convertDocumentWithCaps(ctx, doc, mc)
}

// convertDocumentWithCaps is the caps-injectable variant used by tests.
func convertDocumentWithCaps(ctx context.Context, doc chat.Document, mc modelcaps.ModelCapabilities) ([]openai.ChatCompletionContentPartUnionParam, error) {
	strategy, reason := attachment.Decide(doc, mc)

	switch strategy {
	case attachment.StrategyDrop:
		slog.WarnContext(ctx, "attachment dropped", "reason", reason, "doc", doc.Name)
		return nil, nil

	case attachment.StrategyB64:
		mime := strings.ToLower(doc.MimeType)
		if strings.HasPrefix(mime, "image/") {
			// Build an OpenAI image part with a data URI.
			dataURI := fmt.Sprintf("data:%s;base64,%s",
				doc.MimeType,
				base64.StdEncoding.EncodeToString(doc.Source.InlineData))
			return []openai.ChatCompletionContentPartUnionParam{
				openai.ImageContentPart(openai.ChatCompletionContentPartImageImageURLParam{
					URL: dataURI,
				}),
			}, nil
		}
		// application/pdf and other binary MIMEs: the Chat Completions endpoint has
		// no native document block. Sending raw PDF bytes as base64-in-TXT-envelope
		// is wasteful and meaningless. Drop and warn so the caller can handle it.
		slog.WarnContext(ctx, "oaistream: PDF attachments are not supported on this provider's "+
			"Chat Completions endpoint — dropping",
			"mime", doc.MimeType, "doc", doc.Name)
		return nil, nil

	case attachment.StrategyTXT:
		envelope := attachment.TXTEnvelope(doc.Name, doc.MimeType, doc.Source.InlineText)
		return []openai.ChatCompletionContentPartUnionParam{
			openai.TextContentPart(envelope),
		}, nil

	default:
		return nil, fmt.Errorf("unknown attachment strategy %d", strategy)
	}
}
