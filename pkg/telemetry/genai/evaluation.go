package genai

import (
	"context"

	"go.opentelemetry.io/otel/log"
	"go.opentelemetry.io/otel/log/global"
)

// EvaluationResult is one evaluation outcome emitted as a
// `gen_ai.evaluation.result` log record per the GenAI semconv.
type EvaluationResult struct {
	// Name is the evaluation metric (e.g. "relevance"). Required.
	Name string

	// ScoreLabel is the human-readable verdict (e.g. "passed").
	ScoreLabel string

	// ScoreValue is the optional numeric score.
	ScoreValue    float64
	HasScoreValue bool

	// Explanation is a free-form reason for the score.
	Explanation string

	// ErrorType is set when the evaluation itself failed.
	ErrorType string
}

// EmitEvaluationResult emits a `gen_ai.evaluation.result` log record linked
// to the active span via ctx. No-op when no logger provider is configured.
func EmitEvaluationResult(ctx context.Context, result EvaluationResult) {
	logger := global.GetLoggerProvider().Logger(instrumentationName)

	var rec log.Record
	rec.SetEventName("gen_ai.evaluation.result")
	rec.SetSeverity(log.SeverityInfo)
	rec.SetSeverityText("INFO")

	rec.AddAttributes(log.String(AttrEvaluationName, result.Name))
	if result.ScoreLabel != "" {
		rec.AddAttributes(log.String(AttrEvaluationScoreLabel, result.ScoreLabel))
	}
	if result.HasScoreValue {
		rec.AddAttributes(log.Float64(AttrEvaluationScoreValue, result.ScoreValue))
	}
	if result.Explanation != "" {
		rec.AddAttributes(log.String(AttrEvaluationExplanation, result.Explanation))
	}
	if result.ErrorType != "" {
		rec.AddAttributes(log.String("error.type", result.ErrorType))
	}
	if convID := ConversationIDFromContext(ctx); convID != "" {
		rec.AddAttributes(log.String(AttrConversationID, convID))
	}

	logger.Emit(ctx, rec)
}
