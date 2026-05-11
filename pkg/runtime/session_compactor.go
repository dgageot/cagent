package runtime

import (
	"context"
	"log/slog"

	"github.com/docker/docker-agent/pkg/agent"
	"github.com/docker/docker-agent/pkg/compaction"
	"github.com/docker/docker-agent/pkg/httpclient"
	"github.com/docker/docker-agent/pkg/modelsdev"
	"github.com/docker/docker-agent/pkg/session"
)

// sessionCompactor concentrates session compaction logic that was
// previously scattered across runtime.go (compactWithReason, Summarize,
// preCompactSourceFor, joinPrompts), session_compaction.go (doCompact,
// summaryFromHook, compactionContextLimit, runCompactionAgent), and
// loop.go (compactIfNeeded). Grouping them here makes the compaction
// surface self-contained and independently understandable.
//
// The compactor is a collaborator of [LocalRuntime]: it holds references
// to the runtime's stores and uses callback functions for hook dispatch
// (hooks stay on the runtime because they depend on the per-agent
// [hooks.Executor]).
type sessionCompactor struct {
	runtime *LocalRuntime
}

// Compact runs a session compaction with the supplied reason and
// emits a TokenUsageEvent so the UI immediately reflects the new
// context pressure.
func (c *sessionCompactor) Compact(ctx context.Context, sess *session.Session, additionalPrompt, reason string, events EventSink) {
	r := c.runtime

	ctx = httpclient.ContextWithSessionID(ctx, sess.ID)
	a := r.resolveSessionAgent(sess)

	source := preCompactSourceFor(reason)
	skip, msg, extraPrompt := r.executePreCompactHooks(ctx, sess, a, source, events)
	if skip {
		slog.WarnContext(ctx, "pre_compact hook signalled skip",
			"agent", a.Name(), "session_id", sess.ID, "source", source, "reason", msg)
		if msg != "" {
			events.Emit(Warning(msg, a.Name()))
		}
		return
	}
	additionalPrompt = joinPrompts(additionalPrompt, extraPrompt)

	r.doCompact(ctx, sess, a, additionalPrompt, reason, events)

	modelID := r.getEffectiveModelID(a)
	var contextLimit int64
	if m, err := r.modelsStore.GetModel(ctx, modelID); err == nil && m != nil {
		contextLimit = int64(m.Limit.Context)
	}
	events.Emit(NewTokenUsageEvent(sess.ID, a.Name(), SessionUsage(sess, contextLimit)))
}

// CompactIfNeeded estimates the token impact of tool results added since
// messageCountBefore and triggers proactive compaction when the estimated
// total exceeds 90% of the context window. This prevents sending an
// oversized request on the next iteration.
func (c *sessionCompactor) CompactIfNeeded(
	ctx context.Context,
	sess *session.Session,
	a *agent.Agent,
	m *modelsdev.Model,
	contextLimit int64,
	messageCountBefore int,
	events EventSink,
) {
	if m == nil || !c.runtime.sessionCompaction || contextLimit <= 0 {
		return
	}

	newMessages := sess.GetAllMessages()[messageCountBefore:]
	var addedTokens int64
	for _, msg := range newMessages {
		addedTokens += compaction.EstimateMessageTokens(&msg.Message)
	}

	if !compaction.ShouldCompact(sess.InputTokens, sess.OutputTokens, addedTokens, contextLimit) {
		return
	}

	slog.InfoContext(ctx, "Proactive compaction: tool results pushed estimated context past 90% threshold",
		"agent", a.Name(),
		"input_tokens", sess.InputTokens,
		"output_tokens", sess.OutputTokens,
		"added_estimated_tokens", addedTokens,
		"estimated_total", sess.InputTokens+sess.OutputTokens+addedTokens,
		"context_limit", contextLimit,
	)
	c.Compact(ctx, sess, "", compactionReasonThreshold, events)
}

// CompactIfOverThreshold triggers compaction when the session's
// current token usage exceeds the 90% context threshold. Unlike
// CompactIfNeeded, this does not estimate additional tokens from new
// messages — it checks the session's existing InputTokens and
// OutputTokens against the model's context limit.
func (c *sessionCompactor) CompactIfOverThreshold(ctx context.Context, sess *session.Session, m *modelsdev.Model, events EventSink) {
	if m == nil || !c.runtime.sessionCompaction {
		return
	}
	contextLimit := int64(m.Limit.Context)
	if contextLimit <= 0 {
		return
	}
	if compaction.ShouldCompact(sess.InputTokens, sess.OutputTokens, 0, contextLimit) {
		c.Compact(ctx, sess, "", compactionReasonThreshold, events)
	}
}

// preCompactSourceFor maps the canonical compaction reason onto the
// hooks.Input.Source string surfaced by the pre_compact hook.
func preCompactSourceFor(reason string) string {
	switch reason {
	case compactionReasonThreshold:
		return "auto"
	case compactionReasonOverflow:
		return "overflow"
	case compactionReasonManual:
		return "manual"
	default:
		return reason
	}
}

// joinPrompts concatenates two non-empty prompt fragments with a blank
// line, returning whichever is non-empty when the other isn't.
func joinPrompts(a, b string) string {
	switch {
	case a == "":
		return b
	case b == "":
		return a
	default:
		return a + "\n\n" + b
	}
}
