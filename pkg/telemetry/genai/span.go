package genai

import (
	"context"
	"net/url"
	"slices"
	"strconv"
	"sync"
	"time"

	"go.opentelemetry.io/otel"
	"go.opentelemetry.io/otel/attribute"
	"go.opentelemetry.io/otel/codes"
	"go.opentelemetry.io/otel/metric"
	"go.opentelemetry.io/otel/trace"
	tracenoop "go.opentelemetry.io/otel/trace/noop"
)

// ChatRequest carries the inputs needed to start a `chat {model}` span.
type ChatRequest struct {
	// Provider is the GenAI provider name (use the Provider* constants).
	Provider string

	// Model is the requested model identifier; empty produces span name "chat".
	Model string

	// Stream is true if the request is streaming.
	Stream bool

	// ServerAddress / ServerPort identify the GenAI endpoint when known.
	ServerAddress string
	ServerPort    int

	// Sampling parameters. Zero values are treated as unset.
	MaxTokens        int
	Temperature      float64
	TopP             float64
	TopK             float64
	FrequencyPenalty float64
	PresencePenalty  float64
	Seed             int
	StopSequences    []string
	ChoiceCount      int

	// Has* flags disambiguate "explicitly zero" from "unset" for floats.
	HasTemperature bool
	HasTopP        bool
	HasTopK        bool
	HasFreqPenalty bool
	HasPresPenalty bool
}

// ServerAddressFromURL extracts host and port from a URL string.
func ServerAddressFromURL(raw string) (string, int) {
	if raw == "" {
		return "", 0
	}
	u, err := url.Parse(raw)
	if err != nil || u.Host == "" {
		return "", 0
	}
	port, _ := strconv.Atoi(u.Port())
	return u.Hostname(), port
}

// ChatSpan is the handle returned by StartChat.
type ChatSpan struct {
	span      trace.Span
	provider  string
	model     string
	startedAt time.Time
	metricCtx context.Context //nolint:containedctx // intentional: needed for OTel exemplar attribution at End time

	mu            sync.Mutex
	ended         bool
	responseModel string
	finishReasons []string
	usageRecorded bool
	usage         chatUsage
	errType       string

	// Streaming metrics state.
	firstChunkAt   time.Time
	prevChunkAt    time.Time
	chunkDurations []float64
}

type chatUsage struct {
	inputTokens        int64
	outputTokens       int64
	cacheReadInput     int64
	cacheCreationInput int64
	reasoningOutput    int64
}

// StartChat begins a CLIENT-kind `chat {model}` span. Callers MUST call
// ChatSpan.End to flush the span and metrics.
func StartChat(ctx context.Context, req ChatRequest) (context.Context, *ChatSpan) {
	tracer := otel.Tracer(instrumentationName)

	name := OperationChat
	if req.Model != "" {
		name = OperationChat + " " + req.Model
	}

	attrs := []attribute.KeyValue{
		attribute.String(AttrOperationName, OperationChat),
		attribute.String(AttrProviderName, req.Provider),
		attribute.Bool(AttrRequestStream, req.Stream),
	}
	if req.Model != "" {
		attrs = append(attrs, attribute.String(AttrRequestModel, req.Model))
	}
	if req.ServerAddress != "" {
		attrs = append(attrs, attribute.String("server.address", req.ServerAddress))
		if req.ServerPort > 0 {
			attrs = append(attrs, attribute.Int("server.port", req.ServerPort))
		}
	}
	if req.MaxTokens > 0 {
		attrs = append(attrs, attribute.Int(AttrRequestMaxTokens, req.MaxTokens))
	}
	if req.HasTemperature {
		attrs = append(attrs, attribute.Float64(AttrRequestTemperature, req.Temperature))
	}
	if req.HasTopP {
		attrs = append(attrs, attribute.Float64(AttrRequestTopP, req.TopP))
	}
	if req.HasTopK {
		attrs = append(attrs, attribute.Float64(AttrRequestTopK, req.TopK))
	}
	if req.HasFreqPenalty {
		attrs = append(attrs, attribute.Float64(AttrRequestFrequencyPenalty, req.FrequencyPenalty))
	}
	if req.HasPresPenalty {
		attrs = append(attrs, attribute.Float64(AttrRequestPresencePenalty, req.PresencePenalty))
	}
	if req.Seed != 0 {
		attrs = append(attrs, attribute.Int(AttrRequestSeed, req.Seed))
	}
	if len(req.StopSequences) > 0 {
		attrs = append(attrs, attribute.StringSlice(AttrRequestStopSequences, req.StopSequences))
	}
	if req.ChoiceCount > 0 && req.ChoiceCount != 1 {
		attrs = append(attrs, attribute.Int(AttrRequestChoiceCount, req.ChoiceCount))
	}
	if conv, ok := conversationAttribute(ctx); ok {
		attrs = append(attrs, conv)
	}

	ctx, span := tracer.Start(ctx, name,
		trace.WithSpanKind(trace.SpanKindClient),
		trace.WithAttributes(attrs...),
	)

	return ctx, &ChatSpan{
		span:      span,
		provider:  req.Provider,
		model:     req.Model,
		startedAt: time.Now(),
		metricCtx: ctx,
	}
}

// SetAttributes adds extra attributes to the span.
func (s *ChatSpan) SetAttributes(attrs ...attribute.KeyValue) {
	if s == nil {
		return
	}
	s.span.SetAttributes(attrs...)
}

// SetResponseModel records gen_ai.response.model.
func (s *ChatSpan) SetResponseModel(model string) {
	if s == nil || model == "" {
		return
	}
	s.mu.Lock()
	s.responseModel = model
	s.mu.Unlock()
	s.span.SetAttributes(attribute.String(AttrResponseModel, model))
}

// SetResponseID records gen_ai.response.id.
func (s *ChatSpan) SetResponseID(id string) {
	if s == nil || id == "" {
		return
	}
	s.span.SetAttributes(attribute.String(AttrResponseID, id))
}

// AddFinishReason accumulates a finish reason; duplicates are ignored.
func (s *ChatSpan) AddFinishReason(reason string) {
	if s == nil || reason == "" {
		return
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if slices.Contains(s.finishReasons, reason) {
		return
	}
	s.finishReasons = append(s.finishReasons, reason)
}

// RecordUsage stores the token usage. The Anthropic input-token sum (raw +
// cache_read + cache_creation) is applied at End time.
func (s *ChatSpan) RecordUsage(inputTokens, outputTokens, cacheReadInput, cacheCreationInput, reasoningOutput int64) {
	if s == nil {
		return
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	s.usage.inputTokens = inputTokens
	s.usage.outputTokens = outputTokens
	s.usage.cacheReadInput = cacheReadInput
	s.usage.cacheCreationInput = cacheCreationInput
	s.usage.reasoningOutput = reasoningOutput
	s.usageRecorded = true
}

// MarkChunk records the timing of a streamed output chunk.
func (s *ChatSpan) MarkChunk() {
	if s == nil {
		return
	}
	now := time.Now()
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.firstChunkAt.IsZero() {
		s.firstChunkAt = now
	} else {
		s.chunkDurations = append(s.chunkDurations, now.Sub(s.prevChunkAt).Seconds())
	}
	s.prevChunkAt = now
}

// RecordError marks the span as failed. errType may be empty; ClassifyError
// derives a value when so.
func (s *ChatSpan) RecordError(err error, errType string) {
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

// End closes the span, flushes accumulated state, and records duration +
// token-usage histograms. Idempotent.
func (s *ChatSpan) End() {
	if s == nil {
		return
	}
	s.mu.Lock()
	if s.ended {
		s.mu.Unlock()
		return
	}
	s.ended = true
	finishReasons := append([]string(nil), s.finishReasons...)
	usage := s.usage
	usageRecorded := s.usageRecorded
	errType := s.errType
	firstChunkAt := s.firstChunkAt
	chunkDurations := append([]float64(nil), s.chunkDurations...)
	s.mu.Unlock()

	if len(finishReasons) > 0 {
		s.span.SetAttributes(attribute.StringSlice(AttrResponseFinishReasons, finishReasons))
	}
	if !firstChunkAt.IsZero() {
		ttfc := firstChunkAt.Sub(s.startedAt).Seconds()
		s.span.SetAttributes(attribute.Float64(AttrResponseTimeToFirstChunk, ttfc))
	}
	if usageRecorded {
		// Anthropic reports input_tokens excluding cache; spec requires the
		// inclusive total on gen_ai.usage.input_tokens.
		spanInputTokens := usage.inputTokens
		if s.provider == ProviderAnthropic {
			spanInputTokens += usage.cacheReadInput + usage.cacheCreationInput
		}
		spanAttrs := []attribute.KeyValue{
			attribute.Int64(AttrUsageInputTokens, spanInputTokens),
			attribute.Int64(AttrUsageOutputTokens, usage.outputTokens),
		}
		if usage.cacheReadInput > 0 {
			spanAttrs = append(spanAttrs, attribute.Int64(AttrUsageCacheReadInputTokens, usage.cacheReadInput))
		}
		if usage.cacheCreationInput > 0 {
			spanAttrs = append(spanAttrs, attribute.Int64(AttrUsageCacheCreationInputTokens, usage.cacheCreationInput))
		}
		if usage.reasoningOutput > 0 {
			spanAttrs = append(spanAttrs, attribute.Int64(AttrUsageReasoningOutputTokens, usage.reasoningOutput))
		}
		s.span.SetAttributes(spanAttrs...)
	}

	s.span.End()

	insts := getInstruments()
	if insts == nil {
		return
	}

	commonAttrs := []attribute.KeyValue{
		attribute.String(AttrOperationName, OperationChat),
		attribute.String(AttrProviderName, s.provider),
	}
	// gen_ai.request.model is required by spec but unbounded in practice;
	// canonicalise at the collector if backend cardinality matters.
	if s.model != "" {
		commonAttrs = append(commonAttrs, attribute.String(AttrRequestModel, s.model))
	}

	durationAttrs := append([]attribute.KeyValue(nil), commonAttrs...)
	if errType != "" {
		durationAttrs = append(durationAttrs, attribute.String("error.type", errType))
	}
	if insts.clientOperationDuration != nil {
		insts.clientOperationDuration.Record(s.metricCtx, time.Since(s.startedAt).Seconds(),
			metric.WithAttributes(durationAttrs...),
		)
	}

	if !firstChunkAt.IsZero() && insts.clientOperationTTFC != nil {
		insts.clientOperationTTFC.Record(s.metricCtx, firstChunkAt.Sub(s.startedAt).Seconds(),
			metric.WithAttributes(commonAttrs...),
		)
	}
	if insts.clientOperationTimePerChunk != nil {
		for _, d := range chunkDurations {
			insts.clientOperationTimePerChunk.Record(s.metricCtx, d,
				metric.WithAttributes(commonAttrs...),
			)
		}
	}

	if usageRecorded && insts.clientTokenUsage != nil {
		recordTokenMetric := func(tokenType string, value int64) {
			if value <= 0 {
				return
			}
			tokenAttrs := append([]attribute.KeyValue(nil), commonAttrs...)
			tokenAttrs = append(tokenAttrs, attribute.String(AttrTokenType, tokenType))
			insts.clientTokenUsage.Record(s.metricCtx, value,
				metric.WithAttributes(tokenAttrs...),
			)
		}
		// Per-token-type metrics use raw provider values so summing across
		// types reconstructs the true total without double-counting.
		recordTokenMetric(TokenTypeInput, usage.inputTokens)
		recordTokenMetric(TokenTypeOutput, usage.outputTokens)
		recordTokenMetric(TokenTypeCacheRead, usage.cacheReadInput)
		recordTokenMetric(TokenTypeCacheCreation, usage.cacheCreationInput)
		recordTokenMetric(TokenTypeReasoning, usage.reasoningOutput)
	}
}

// Span returns the underlying OTel span. Returns a no-op span when the
// receiver is nil so callers don't have to nil-check.
func (s *ChatSpan) Span() trace.Span {
	if s == nil {
		return tracenoop.Span{}
	}
	return s.span
}
