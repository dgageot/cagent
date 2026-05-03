package genai

import (
	"context"
	"errors"
	"net"
	"strings"

	"go.opentelemetry.io/otel/attribute"
)

// ErrorTypeOther is the OTel-mandated fallback for `error.type` when no
// classifier matches. The spec requires `_OTHER` rather than a Go type
// name so backends can rely on a bounded cardinality.
const ErrorTypeOther = "_OTHER"

// ClassifyError maps a provider error to a low-cardinality `error.type`
// value suitable for span and metric attributes. Falls back to
// `_OTHER` (the spec-defined sentinel) when the error does not match any
// known pattern.
//
// Spec leaves the value space open for callers — these strings are picked
// for cross-provider comparability on dashboards.
func ClassifyError(err error) string {
	if err == nil {
		return ""
	}
	switch {
	case errors.Is(err, context.Canceled):
		return "context_canceled"
	case errors.Is(err, context.DeadlineExceeded):
		return "deadline_exceeded"
	}

	msg := strings.ToLower(err.Error())
	switch {
	case strings.Contains(msg, "context length") || strings.Contains(msg, "context_length"):
		// Bare "max_tokens" matches too eagerly: validation errors
		// like `max_tokens must be > 0` and "model X does not
		// support max_tokens" both contain the token but are not
		// context overflows. Stick to the unambiguous phrases.
		return "context_length_exceeded"
	case strings.Contains(msg, "rate limit") || strings.Contains(msg, "429"):
		return "rate_limit"
	case strings.Contains(msg, "401") || strings.Contains(msg, "unauthorized") || strings.Contains(msg, "authentication"):
		return "auth"
	case strings.Contains(msg, "403") || strings.Contains(msg, "forbidden") || strings.Contains(msg, "permission"):
		return "forbidden"
	case strings.Contains(msg, "content policy") || strings.Contains(msg, "content filter") || strings.Contains(msg, "safety"):
		return "content_policy"
	}

	var netErr net.Error
	if errors.As(err, &netErr) {
		if netErr.Timeout() {
			return "network_timeout"
		}
		return "network"
	}

	return ErrorTypeOther
}

// applyExtraAttribute converts a StreamAttributer KeyValue into an OTel
// attribute and applies it to the span. Unsupported value types are
// dropped silently — telemetry must never crash request paths.
func applyExtraAttribute(span *ChatSpan, kv KeyValue) {
	if span == nil || kv.Key == "" {
		return
	}
	switch v := kv.Value.(type) {
	case string:
		span.SetAttributes(attribute.String(kv.Key, v))
	case bool:
		span.SetAttributes(attribute.Bool(kv.Key, v))
	case int:
		span.SetAttributes(attribute.Int(kv.Key, v))
	case int64:
		span.SetAttributes(attribute.Int64(kv.Key, v))
	case float64:
		span.SetAttributes(attribute.Float64(kv.Key, v))
	case []string:
		span.SetAttributes(attribute.StringSlice(kv.Key, v))
	}
}
