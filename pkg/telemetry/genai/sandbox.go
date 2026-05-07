package genai

import (
	"context"
	"strings"
	"sync"
	"time"

	"go.opentelemetry.io/otel"
	"go.opentelemetry.io/otel/attribute"
	"go.opentelemetry.io/otel/codes"
	"go.opentelemetry.io/otel/metric"
	"go.opentelemetry.io/otel/propagation"
	"go.opentelemetry.io/otel/trace"
)

// envCarrier adapts an env-var key/value map to OTel's TextMapCarrier so
// the propagator can write traceparent / tracestate / baggage into a
// subprocess's environment. Keys are uppercased on Set.
type envCarrier map[string]string

func (c envCarrier) Get(key string) string { return c[strings.ToUpper(key)] }
func (c envCarrier) Set(key, value string) { c[strings.ToUpper(key)] = value }
func (c envCarrier) Keys() []string {
	keys := make([]string, 0, len(c))
	for k := range c {
		keys = append(keys, k)
	}
	return keys
}

var _ propagation.TextMapCarrier = envCarrier{}

// InjectSandboxEnv returns docker-style `-e KEY=VALUE` flags carrying the
// W3C trace context for the current span.
func InjectSandboxEnv(ctx context.Context) []string {
	carrier := envCarrier{}
	otel.GetTextMapPropagator().Inject(ctx, carrier)
	if len(carrier) == 0 {
		return nil
	}
	flags := make([]string, 0, 2*len(carrier))
	for k, v := range carrier {
		flags = append(flags, "-e", k+"="+v)
	}
	return flags
}

// InjectTraceContextEnv returns `KEY=VALUE` env-var strings carrying the
// W3C trace context for the current span. Companion to InjectSandboxEnv
// for direct subprocess spawns.
func InjectTraceContextEnv(ctx context.Context) []string {
	carrier := envCarrier{}
	otel.GetTextMapPropagator().Inject(ctx, carrier)
	if len(carrier) == 0 {
		return nil
	}
	out := make([]string, 0, len(carrier))
	for k, v := range carrier {
		out = append(out, k+"="+v)
	}
	return out
}

// SandboxSpan handles the lifecycle of a sandbox.exec span and the matching
// duration histogram.
type SandboxSpan struct {
	span      trace.Span
	metricCtx context.Context //nolint:containedctx // intentional: needed for OTel exemplar attribution at End time
	startedAt time.Time
	runtime   string

	mu       sync.Mutex
	exitCode int
	hasExit  bool
	errType  string
	ended    bool
}

// SandboxOptions configures a sandbox.exec span.
type SandboxOptions struct {
	// Runtime is a short label identifying the sandbox backend (e.g. "docker").
	Runtime string

	// Image is the container/pod image when known.
	Image string

	// Container is the container/pod identifier when known.
	Container string

	// AgentName is the agent being executed in the sandbox.
	AgentName string
}

// StartSandboxExec opens a `sandbox.exec` INTERNAL span.
func StartSandboxExec(ctx context.Context, opts SandboxOptions) (context.Context, *SandboxSpan) {
	tracer := otel.Tracer(instrumentationName)
	attrs := []attribute.KeyValue{}
	if opts.Runtime != "" {
		attrs = append(attrs, attribute.String(AttrSandboxRuntime, opts.Runtime))
	}
	if opts.Image != "" {
		attrs = append(attrs, attribute.String(AttrSandboxImage, opts.Image))
	}
	if opts.Container != "" {
		attrs = append(attrs, attribute.String(AttrSandboxContainer, opts.Container))
	}
	if opts.AgentName != "" {
		attrs = append(attrs, attribute.String(AttrAgentNameRuntime, opts.AgentName))
	}
	if conv, ok := conversationAttribute(ctx); ok {
		attrs = append(attrs, conv)
	}
	ctx, span := tracer.Start(ctx, "sandbox.exec",
		trace.WithSpanKind(trace.SpanKindInternal),
		trace.WithAttributes(attrs...),
	)
	return ctx, &SandboxSpan{span: span, metricCtx: ctx, startedAt: time.Now(), runtime: opts.Runtime}
}

// SetExitCode records the subprocess exit code.
func (s *SandboxSpan) SetExitCode(code int) {
	if s == nil {
		return
	}
	s.mu.Lock()
	s.exitCode = code
	s.hasExit = true
	s.mu.Unlock()
	s.span.SetAttributes(attribute.Int(AttrSandboxExitCode, code))
}

// RecordError marks the span as failed.
func (s *SandboxSpan) RecordError(err error, errType string) {
	if s == nil || err == nil {
		return
	}
	if errType == "" {
		errType = ClassifyError(err)
	}
	s.mu.Lock()
	s.errType = errType
	s.mu.Unlock()
	s.span.RecordError(err)
	s.span.SetStatus(codes.Error, err.Error())
	s.span.SetAttributes(attribute.String("error.type", errType))
}

// End closes the span and records the sandbox.exec.duration histogram.
func (s *SandboxSpan) End() {
	if s == nil {
		return
	}
	s.mu.Lock()
	if s.ended {
		s.mu.Unlock()
		return
	}
	s.ended = true
	errType := s.errType
	s.mu.Unlock()

	s.span.End()

	hist := getSandboxDurationHistogram()
	if hist == nil {
		return
	}
	attrs := []attribute.KeyValue{}
	if s.runtime != "" {
		attrs = append(attrs, attribute.String(AttrSandboxRuntime, s.runtime))
	}
	if errType != "" {
		attrs = append(attrs, attribute.String("error.type", errType))
	}
	hist.Record(s.metricCtx, time.Since(s.startedAt).Seconds(),
		metric.WithAttributes(attrs...),
	)
}

var (
	sandboxDurationOnce sync.Once
	sandboxDurationHist metric.Float64Histogram
)

func getSandboxDurationHistogram() metric.Float64Histogram {
	sandboxDurationOnce.Do(func() {
		meter := otel.Meter(instrumentationName)
		h, err := meter.Float64Histogram(
			"cagent.sandbox.exec.duration",
			metric.WithUnit("s"),
			metric.WithDescription("Time the host side spent waiting for a sandbox exec invocation to complete."),
			metric.WithExplicitBucketBoundaries(metricBucketsDuration...),
		)
		if err != nil {
			return
		}
		sandboxDurationHist = h
	})
	return sandboxDurationHist
}
