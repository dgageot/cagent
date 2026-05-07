package genai

import (
	"context"
	"errors"
	"net"
	"strings"

	"go.opentelemetry.io/otel/attribute"
)

// ErrorTypeOther is the OTel-mandated fallback for `error.type`.
const ErrorTypeOther = "_OTHER"

// ClassifyError maps a provider error to a low-cardinality `error.type` value.
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

	// Prefer a structured status-code probe before substring matching
	// to avoid tripping on "401" / "403" / "429" appearing in unrelated
	// error message fragments.
	if t := classifyByStatusCode(err); t != "" {
		return t
	}

	msg := strings.ToLower(err.Error())
	switch {
	case strings.Contains(msg, "context length") || strings.Contains(msg, "context_length"):
		// Avoid bare "max_tokens" — it matches validation errors too.
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

// classifyByStatusCode returns a low-cardinality `error.type` when err exposes
// a `StatusCode() int` method matching a handled case, otherwise "".
func classifyByStatusCode(err error) string {
	var sc interface{ StatusCode() int }
	if !errors.As(err, &sc) {
		return ""
	}
	switch sc.StatusCode() {
	case 401:
		return "auth"
	case 403:
		return "forbidden"
	case 429:
		return "rate_limit"
	}
	return ""
}

// applyExtraAttribute applies a StreamAttributer KeyValue to the span. Unsupported
// value types are dropped silently.
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
