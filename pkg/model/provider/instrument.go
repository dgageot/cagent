package provider

import (
	"context"

	"go.opentelemetry.io/otel"
	"go.opentelemetry.io/otel/attribute"
	"go.opentelemetry.io/otel/codes"
	"go.opentelemetry.io/otel/trace"

	"github.com/docker/docker-agent/pkg/chat"
	"github.com/docker/docker-agent/pkg/model/provider/base"
	"github.com/docker/docker-agent/pkg/rag/types"
	"github.com/docker/docker-agent/pkg/telemetry/genai"
	"github.com/docker/docker-agent/pkg/tools"
)

// unwrapProvider returns the leaf provider underneath any instrumentation
// wrappers.
func unwrapProvider(p Provider) Provider {
	for {
		u, ok := p.(interface{ Unwrap() Provider })
		if !ok {
			return p
		}
		p = u.Unwrap()
	}
}

// instrumentProvider wraps the leaf provider with GenAI semconv spans and
// metrics. The returned wrapper satisfies exactly the same interfaces as
// the inner provider so capability assertions like `p.(EmbeddingProvider)`
// still behave correctly.
func instrumentProvider(p Provider) Provider {
	if p == nil {
		return nil
	}

	tc := &tracedChat{inner: p}

	bep, isBatchEmbed := p.(BatchEmbeddingProvider)
	ep, isEmbed := p.(EmbeddingProvider)
	rp, isRerank := p.(RerankingProvider)

	switch {
	case isBatchEmbed && isRerank:
		return &tracedBatchEmbedRerank{tracedChat: tc, batchEmbed: bep, rerank: rp}
	case isBatchEmbed:
		return &tracedBatchEmbed{tracedChat: tc, batchEmbed: bep}
	case isEmbed && isRerank:
		return &tracedEmbedRerank{tracedChat: tc, embed: ep, rerank: rp}
	case isEmbed:
		return &tracedEmbed{tracedChat: tc, embed: ep}
	case isRerank:
		return &tracedRerank{tracedChat: tc, rerank: rp}
	default:
		return tc
	}
}

// tracedChat is the base wrapper. Only CreateChatCompletionStream adds
// behaviour; everything else delegates.
type tracedChat struct {
	inner Provider
}

func (t *tracedChat) ID() string              { return t.inner.ID() }
func (t *tracedChat) BaseConfig() base.Config { return t.inner.BaseConfig() }

// Unwrap returns the wrapped provider.
func (t *tracedChat) Unwrap() Provider { return t.inner }

func (t *tracedChat) CreateChatCompletionStream(ctx context.Context, messages []chat.Message, requestTools []tools.Tool) (chat.MessageStream, error) {
	cfg := t.inner.BaseConfig()
	req := genai.ChatRequest{
		Provider: genai.ProviderNameForConfig(cfg.ModelConfig.Provider),
		Model:    cfg.ModelConfig.Model,
		Stream:   true,
	}
	// Pointer fields on ModelConfig distinguish unset from zero.
	if mc := cfg.ModelConfig.MaxTokens; mc != nil {
		req.MaxTokens = int(*mc)
	}
	if t := cfg.ModelConfig.Temperature; t != nil {
		req.Temperature = *t
		req.HasTemperature = true
	}
	if tp := cfg.ModelConfig.TopP; tp != nil {
		req.TopP = *tp
		req.HasTopP = true
	}
	chatCtx, span := genai.StartChat(ctx, req)

	genai.SetInputMessages(span, messages)
	genai.SetToolDefinitions(span, requestTools)

	stream, err := t.inner.CreateChatCompletionStream(chatCtx, messages, requestTools)
	if err != nil {
		span.RecordError(err, genai.ClassifyError(err))
		span.End()
		return nil, err
	}
	return genai.WrapStream(span, stream), nil
}

// embeddingRequestForConfig builds an EmbeddingRequest from the inner
// provider's BaseConfig.
func (t *tracedChat) embeddingRequestForConfig(batchSize int) genai.EmbeddingRequest {
	cfg := t.inner.BaseConfig()
	return genai.EmbeddingRequest{
		Provider:  genai.ProviderNameForConfig(cfg.ModelConfig.Provider),
		Model:     cfg.ModelConfig.Model,
		BatchSize: batchSize,
	}
}

// rerankSpan opens a `rerank` CLIENT span. There is no spec span for
// rerank yet, so custom attributes use the `cagent.*` namespace.
func (t *tracedChat) rerankSpan(ctx context.Context, docCount int) (context.Context, trace.Span) {
	cfg := t.inner.BaseConfig()
	tracer := otel.Tracer("github.com/docker/docker-agent/pkg/model/provider")
	attrs := []attribute.KeyValue{
		attribute.String(genai.AttrProviderName, genai.ProviderNameForConfig(cfg.ModelConfig.Provider)),
		attribute.String(genai.AttrRequestModel, cfg.ModelConfig.Model),
		attribute.Int("cagent.rerank.document_count", docCount),
	}
	if convID := genai.ConversationIDFromContext(ctx); convID != "" {
		attrs = append(attrs, attribute.String(genai.AttrConversationID, convID))
	}
	return tracer.Start(ctx, "rerank",
		trace.WithSpanKind(trace.SpanKindClient),
		trace.WithAttributes(attrs...),
	)
}

// wrapEmbedding wraps a single-input embedding call with a spec
// `embeddings {model}` span.
func wrapEmbedding(ctx context.Context, req genai.EmbeddingRequest, fn func(context.Context) (*base.EmbeddingResult, error)) (*base.EmbeddingResult, error) {
	ctx, span := genai.StartEmbedding(ctx, req)
	defer span.End()
	res, err := fn(ctx)
	if err != nil {
		span.RecordError(err, "")
		return nil, err
	}
	if res != nil {
		span.SetInputTokens(res.InputTokens)
		span.SetDimensions(len(res.Embedding))
	}
	return res, nil
}

// wrapBatchEmbedding wraps a batch embedding call.
func wrapBatchEmbedding(ctx context.Context, req genai.EmbeddingRequest, fn func(context.Context) (*base.BatchEmbeddingResult, error)) (*base.BatchEmbeddingResult, error) {
	ctx, span := genai.StartEmbedding(ctx, req)
	defer span.End()
	res, err := fn(ctx)
	if err != nil {
		span.RecordError(err, "")
		return nil, err
	}
	if res != nil {
		span.SetInputTokens(res.InputTokens)
		if len(res.Embeddings) > 0 {
			span.SetDimensions(len(res.Embeddings[0]))
		}
	}
	return res, nil
}

// wrapRerank wraps a Rerank call with a `rerank` CLIENT span.
func (t *tracedChat) wrapRerank(ctx context.Context, query string, documents []types.Document, criteria string, fn func(context.Context, string, []types.Document, string) ([]float64, error)) ([]float64, error) {
	ctx, span := t.rerankSpan(ctx, len(documents))
	defer span.End()
	scores, err := fn(ctx, query, documents, criteria)
	if err != nil {
		span.RecordError(err)
		span.SetStatus(codes.Error, err.Error())
		span.SetAttributes(attribute.String("error.type", genai.ClassifyError(err)))
		return nil, err
	}
	return scores, nil
}

// tracedRerank adds RerankingProvider.
type tracedRerank struct {
	*tracedChat

	rerank RerankingProvider
}

func (t *tracedRerank) Rerank(ctx context.Context, query string, documents []types.Document, criteria string) ([]float64, error) {
	return t.wrapRerank(ctx, query, documents, criteria, t.rerank.Rerank)
}

// tracedEmbed satisfies EmbeddingProvider.
type tracedEmbed struct {
	*tracedChat

	embed EmbeddingProvider
}

func (t *tracedEmbed) CreateEmbedding(ctx context.Context, text string) (*base.EmbeddingResult, error) {
	return wrapEmbedding(ctx, t.embeddingRequestForConfig(0), func(ctx context.Context) (*base.EmbeddingResult, error) {
		return t.embed.CreateEmbedding(ctx, text)
	})
}

// tracedEmbedRerank satisfies EmbeddingProvider and RerankingProvider.
type tracedEmbedRerank struct {
	*tracedChat

	embed  EmbeddingProvider
	rerank RerankingProvider
}

func (t *tracedEmbedRerank) CreateEmbedding(ctx context.Context, text string) (*base.EmbeddingResult, error) {
	return wrapEmbedding(ctx, t.embeddingRequestForConfig(0), func(ctx context.Context) (*base.EmbeddingResult, error) {
		return t.embed.CreateEmbedding(ctx, text)
	})
}

func (t *tracedEmbedRerank) Rerank(ctx context.Context, query string, documents []types.Document, criteria string) ([]float64, error) {
	return t.wrapRerank(ctx, query, documents, criteria, t.rerank.Rerank)
}

// tracedBatchEmbed satisfies BatchEmbeddingProvider.
type tracedBatchEmbed struct {
	*tracedChat

	batchEmbed BatchEmbeddingProvider
}

func (t *tracedBatchEmbed) CreateEmbedding(ctx context.Context, text string) (*base.EmbeddingResult, error) {
	return wrapEmbedding(ctx, t.embeddingRequestForConfig(0), func(ctx context.Context) (*base.EmbeddingResult, error) {
		return t.batchEmbed.CreateEmbedding(ctx, text)
	})
}

func (t *tracedBatchEmbed) CreateBatchEmbedding(ctx context.Context, texts []string) (*base.BatchEmbeddingResult, error) {
	return wrapBatchEmbedding(ctx, t.embeddingRequestForConfig(len(texts)), func(ctx context.Context) (*base.BatchEmbeddingResult, error) {
		return t.batchEmbed.CreateBatchEmbedding(ctx, texts)
	})
}

// tracedBatchEmbedRerank satisfies BatchEmbeddingProvider and RerankingProvider.
type tracedBatchEmbedRerank struct {
	*tracedChat

	batchEmbed BatchEmbeddingProvider
	rerank     RerankingProvider
}

func (t *tracedBatchEmbedRerank) CreateEmbedding(ctx context.Context, text string) (*base.EmbeddingResult, error) {
	return wrapEmbedding(ctx, t.embeddingRequestForConfig(0), func(ctx context.Context) (*base.EmbeddingResult, error) {
		return t.batchEmbed.CreateEmbedding(ctx, text)
	})
}

func (t *tracedBatchEmbedRerank) CreateBatchEmbedding(ctx context.Context, texts []string) (*base.BatchEmbeddingResult, error) {
	return wrapBatchEmbedding(ctx, t.embeddingRequestForConfig(len(texts)), func(ctx context.Context) (*base.BatchEmbeddingResult, error) {
		return t.batchEmbed.CreateBatchEmbedding(ctx, texts)
	})
}

func (t *tracedBatchEmbedRerank) Rerank(ctx context.Context, query string, documents []types.Document, criteria string) ([]float64, error) {
	return t.wrapRerank(ctx, query, documents, criteria, t.rerank.Rerank)
}
