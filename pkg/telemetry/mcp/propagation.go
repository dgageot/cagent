package mcp

import (
	"context"
	"maps"

	"go.opentelemetry.io/otel"
	"go.opentelemetry.io/otel/propagation"
)

// metaCarrier adapts an MCP `params._meta` map (map[string]any) to
// OTel's TextMapCarrier interface.
type metaCarrier struct {
	meta map[string]any
}

func (c metaCarrier) Get(key string) string {
	if c.meta == nil {
		return ""
	}
	v, ok := c.meta[key]
	if !ok {
		return ""
	}
	if s, ok := v.(string); ok {
		return s
	}
	return ""
}

func (c metaCarrier) Set(key, value string) {
	if c.meta == nil {
		return
	}
	c.meta[key] = value
}

func (c metaCarrier) Keys() []string {
	if c.meta == nil {
		return nil
	}
	keys := make([]string, 0, len(c.meta))
	for k, v := range c.meta {
		if _, ok := v.(string); ok {
			keys = append(keys, k)
		}
	}
	return keys
}

// InjectMeta writes the active trace context into an MCP `_meta` map
// (`traceparent`, `tracestate`, `baggage`). No-op when meta is nil.
func InjectMeta(ctx context.Context, meta map[string]any) {
	if meta == nil {
		return
	}
	otel.GetTextMapPropagator().Inject(ctx, metaCarrier{meta: meta})
}

// ExtractMeta reads trace context from an MCP `_meta` map and returns a
// context with the parent span attached.
func ExtractMeta(ctx context.Context, meta map[string]any) context.Context {
	if meta == nil {
		return ctx
	}
	return otel.GetTextMapPropagator().Extract(ctx, metaCarrier{meta: meta})
}

// EnsureMeta returns a non-nil meta map suitable for InjectMeta. m is
// shallow-copied to avoid stale traceparent keys leaking across retries.
func EnsureMeta(m map[string]any) map[string]any {
	if m == nil {
		return map[string]any{}
	}
	out := make(map[string]any, len(m)+3)
	maps.Copy(out, m)
	return out
}

var _ propagation.TextMapCarrier = metaCarrier{}
