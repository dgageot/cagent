package mcp

import (
	"context"
	"errors"
	"testing"

	"github.com/stretchr/testify/assert"
	"go.opentelemetry.io/otel"
	"go.opentelemetry.io/otel/propagation"
	"go.opentelemetry.io/otel/sdk/trace"
	traceapi "go.opentelemetry.io/otel/trace"
)

func TestEnsureMeta(t *testing.T) {
	t.Parallel()
	got := EnsureMeta(nil)
	assert.NotNil(t, got)
	assert.Empty(t, got)

	existing := map[string]any{"foo": "bar"}
	got = EnsureMeta(existing)
	assert.Equal(t, existing, got)
}

func TestInjectExtractRoundTrip(t *testing.T) {
	// Mutates the global propagator; cannot run in parallel.
	prev := otel.GetTextMapPropagator()
	otel.SetTextMapPropagator(propagation.NewCompositeTextMapPropagator(
		propagation.TraceContext{},
		propagation.Baggage{},
	))
	t.Cleanup(func() { otel.SetTextMapPropagator(prev) })

	// Sampled span so traceparent has a non-trivial trace id.
	tp := trace.NewTracerProvider(trace.WithSampler(trace.AlwaysSample()))
	t.Cleanup(func() { _ = tp.Shutdown(t.Context()) })

	parentCtx, parentSpan := tp.Tracer("test").Start(t.Context(), "parent")
	defer parentSpan.End()
	parentSC := traceapi.SpanContextFromContext(parentCtx)

	meta := map[string]any{}
	InjectMeta(parentCtx, meta)
	assert.Contains(t, meta, "traceparent",
		"propagator should have written W3C traceparent into _meta")

	// Extracted child should match the parent's span context.
	childCtx := ExtractMeta(t.Context(), meta)
	extracted := traceapi.SpanContextFromContext(childCtx)
	assert.Equal(t, parentSC.TraceID(), extracted.TraceID())
	assert.Equal(t, parentSC.SpanID(), extracted.SpanID())
}

func TestInjectMetaNilNoOp(t *testing.T) {
	t.Parallel()
	InjectMeta(t.Context(), nil)
}

func TestExtractMetaNilReturnsParent(t *testing.T) {
	t.Parallel()
	got := ExtractMeta(t.Context(), nil)
	assert.Equal(t, t.Context(), got)
}

func TestStartClientReturnsActiveSpan(t *testing.T) {
	// Mutates the global tracer provider; cannot run in parallel.
	tp := trace.NewTracerProvider(trace.WithSampler(trace.AlwaysSample()))
	t.Cleanup(func() { _ = tp.Shutdown(t.Context()) })
	prev := otel.GetTracerProvider()
	otel.SetTracerProvider(tp)
	t.Cleanup(func() { otel.SetTracerProvider(prev) })

	ctx, span := StartClient(t.Context(), CallOptions{
		Method:   MethodToolsCall,
		ToolName: "search-web",
	})
	defer span.End()

	sc := traceapi.SpanContextFromContext(ctx)
	assert.True(t, sc.IsValid(), "context should carry an active span")
}

func TestClassifyError(t *testing.T) {
	t.Parallel()
	assert.Empty(t, ClassifyError(nil))
	assert.Equal(t, "context_canceled", ClassifyError(context.Canceled))
	assert.Equal(t, "deadline_exceeded", ClassifyError(context.DeadlineExceeded))
	assert.Equal(t, "rpc_error", ClassifyError(errors.New("some other error")))
}
