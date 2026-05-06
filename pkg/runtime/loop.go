package runtime

import (
	"context"
	"fmt"
	"log/slog"
	"slices"
	"strings"
	"time"

	"go.opentelemetry.io/otel/attribute"
	"go.opentelemetry.io/otel/codes"
	"go.opentelemetry.io/otel/trace"

	"github.com/docker/docker-agent/pkg/agent"
	"github.com/docker/docker-agent/pkg/chat"
	"github.com/docker/docker-agent/pkg/compaction"
	"github.com/docker/docker-agent/pkg/httpclient"
	"github.com/docker/docker-agent/pkg/model/provider"
	"github.com/docker/docker-agent/pkg/modelsdev"
	"github.com/docker/docker-agent/pkg/runtime/toolexec"
	"github.com/docker/docker-agent/pkg/session"
	"github.com/docker/docker-agent/pkg/telemetry/genai"
	"github.com/docker/docker-agent/pkg/tools"
	"github.com/docker/docker-agent/pkg/tools/builtin"
	bgagent "github.com/docker/docker-agent/pkg/tools/builtin/agent"
)

// registerDefaultTools wires up the built-in tool handlers (delegation,
// background agents, model switching) into the runtime's tool dispatch map.
func (r *LocalRuntime) registerDefaultTools() {
	r.toolMap[builtin.ToolNameTransferTask] = r.handleTaskTransfer
	r.toolMap[builtin.ToolNameHandoff] = r.handleHandoff
	r.toolMap[builtin.ToolNameChangeModel] = r.handleChangeModel
	r.toolMap[builtin.ToolNameRevertModel] = r.handleRevertModel
	r.toolMap[builtin.ToolNameRunSkill] = r.handleRunSkill

	r.bgAgents.RegisterHandlers(func(name string, fn func(context.Context, *session.Session, tools.ToolCall) (*tools.ToolCallResult, error)) {
		r.toolMap[name] = func(ctx context.Context, sess *session.Session, tc tools.ToolCall, _ chan Event) (*tools.ToolCallResult, error) {
			return fn(ctx, sess, tc)
		}
	})
}

// appendSteerAndEmit adds a steer message to the session and emits the corresponding event.
func (r *LocalRuntime) appendSteerAndEmit(sess *session.Session, sm QueuedMessage, events chan<- Event) {
	sess.AddMessage(session.UserMessage(sm.Content, sm.MultiContent...))
	events <- UserMessage(sm.Content, sess.ID, sm.MultiContent, len(sess.Messages)-1)
}

// drainAndEmitSteered drains all messages from the steer queue and injects
// them into the session as individual user messages. When multiple messages
// are drained, a "\n" is appended to the content of every non-last message.
// Some chat templates concatenate consecutive user messages without a
// separator before tokenisation, which would cause trailing/leading word
// fragments from adjacent messages to be glued together. The "\n" prevents
// this without merging the messages into one.
//
// It also snapshots the message count before any messages are added and
// returns it alongside the drained flag so the caller can pass it to
// compactIfNeeded without a separate len(sess.GetAllMessages()) call.
//
// NOTE: the appended \n is persisted in the session message and included in
// UserMessageEvent. This is a deliberate trade-off: because the runtime passes
// chat.Message slices directly to the provider, this is the only injection
// point that doesn't require restructuring. TUI consumers may see a trailing
// newline on non-last steered messages in multi-drain batches.
//
// Returns (true, messageCountBefore) if any messages were drained and emitted;
// (false, 0) otherwise.
func (r *LocalRuntime) drainAndEmitSteered(ctx context.Context, sess *session.Session, events chan<- Event) (bool, int) {
	steered := r.steerQueue.Drain(ctx)
	if len(steered) == 0 {
		return false, 0
	}
	messageCountBefore := len(sess.GetAllMessages())
	for i, sm := range steered {
		if i < len(steered)-1 {
			sm = appendNewlineToQueuedMessage(sm)
		}
		r.appendSteerAndEmit(sess, sm, events)
	}
	return true, messageCountBefore
}

// appendNewlineToQueuedMessage returns sm with "\n" appended to its text
// content; never mutates the caller's slice contents.
// For plain-text messages Content is extended. For multi-content messages
// only the last part is considered: if it is a text part, "\n" is appended
// to it in a shallow copy of the slice. If the last part is not text type
// (e.g. image), sm is returned unchanged — non-text parts carry their own
// provider envelope that acts as a separator.
func appendNewlineToQueuedMessage(sm QueuedMessage) QueuedMessage {
	if len(sm.MultiContent) == 0 {
		sm.Content += "\n"
		return sm
	}
	// Only act if the last part is a text part.
	last := len(sm.MultiContent) - 1
	if sm.MultiContent[last].Type != chat.MessagePartTypeText {
		return sm
	}
	// Shallow-copy the slice so we don't mutate the original.
	parts := append([]chat.MessagePart(nil), sm.MultiContent...)
	parts[last].Text += "\n"
	sm.MultiContent = parts
	return sm
}

// emitHookDrivenShutdown fans out the standard Error /
// notification(level=error) / on_error stanzas when a hook
// (post_tool_use or before_llm_call) signals run termination.
func (r *LocalRuntime) emitHookDrivenShutdown(
	ctx context.Context,
	a *agent.Agent,
	sess *session.Session,
	message string,
	events chan Event,
) {
	if message == "" {
		// aggregate() always populates Result.Message on a deny
		// verdict; the fallback covers any future hook that returns
		// block without a reason.
		message = "Agent terminated by a hook."
	}
	events <- Error(message)
	r.notifyError(ctx, a, sess.ID, message)
}

// finalizeEventChannel performs cleanup at the end of a RunStream goroutine:
// restores the previous elicitation channel, emits the StreamStopped event,
// fires hooks, and closes the events channel.
func (r *LocalRuntime) finalizeEventChannel(ctx context.Context, sess *session.Session, prevElicitationCh, events chan Event) {
	// Swap back the parent's elicitation channel before closing this
	// stream's channel. This prevents a send-on-closed-channel panic
	// and restores elicitation for the parent session.
	r.elicitation.swap(prevElicitationCh)

	defer close(events)

	a := r.resolveSessionAgent(sess)

	// Execute session end hooks with a context that won't be cancelled so
	// cleanup hooks run even when the stream was interrupted (e.g. Ctrl+C).
	r.executeSessionEndHooks(context.WithoutCancel(ctx), sess, a)

	events <- StreamStopped(sess.ID, a.Name())

	r.executeOnUserInputHooks(ctx, sess.ID, "stream stopped")

	r.telemetry.RecordSessionEnd(ctx)
}

// RunStream starts the agent's interaction loop and returns a channel of events.
// The returned channel is closed when the loop terminates (success, error, or
// context cancellation). Each iteration: sends messages to the model, streams
// the response, executes any tool calls, and loops until the model signals stop
// or the iteration limit is reached.
func (r *LocalRuntime) RunStream(ctx context.Context, sess *session.Session) <-chan Event {
	slog.Debug("Starting runtime stream", "agent", r.CurrentAgentName(), "session_id", sess.ID)
	events := make(chan Event, defaultEventChannelCapacity)

	go r.runStreamLoop(ctx, sess, events)
	return r.observe(ctx, sess, events)
}

// runStreamLoop is the body of RunStream. Pulled out of the anonymous
// goroutine so it has a real name in stack traces and is easier to navigate
// in editors.
func (r *LocalRuntime) runStreamLoop(ctx context.Context, sess *session.Session, events chan Event) {
	// Seed the cagent session ID at the run-loop boundary so any
	// gateway-bound HTTP call originating from this loop can correlate
	// back to the originating session. Plumbing happens in
	// pkg/httpclient/userAgentTransport, gated on `X-Cagent-Forward`.
	ctx = httpclient.ContextWithSessionID(ctx, sess.ID)
	r.telemetry.RecordSessionStart(ctx, r.CurrentAgentName(), sess.ID)

	// Seed `gen_ai.conversation.id` into baggage at the session
	// boundary. Every span the runtime, providers, MCP client, RAG,
	// sandbox, evaluation, hooks, and (downstream) any subprocess
	// or remote service create from here on will pick it up
	// automatically without per-helper plumbing — and the value
	// rides over W3C `baggage` so it crosses MCP / sandbox /
	// HTTP boundaries too.
	ctx = genai.WithConversationID(ctx, sess.ID)

	// runtime.session is the root span for one stream. gen_ai.* keys
	// are emitted alongside the legacy `agent` / `session.id` keys
	// so existing dashboards keep matching while spec-aware tooling
	// can filter by `gen_ai.conversation.id` and
	// `cagent.agent.name`. Legacy keys drop out under
	// OTEL_SEMCONV_STABILITY_OPT_IN=gen_ai_latest_experimental.
	sessionAttrs := []attribute.KeyValue{
		attribute.String(genai.AttrConversationID, sess.ID),
		attribute.String(genai.AttrAgentNameRuntime, r.CurrentAgentName()),
	}
	if genai.EmitLegacyAttributes() {
		sessionAttrs = append(sessionAttrs,
			attribute.String("agent", r.CurrentAgentName()),
			attribute.String("session.id", sess.ID),
		)
	}
	ctx, sessionSpan := r.startSpan(ctx, "runtime.session", trace.WithAttributes(sessionAttrs...))
	defer sessionSpan.End()

	// Swap in this stream's events channel for elicitation and save the
	// previous one so it can be restored on teardown. This allows nested
	// RunStream calls to temporarily own elicitation without losing the
	// parent's channel.
	prevElicitationCh := r.elicitation.swap(events)

	a := r.resolveSessionAgent(sess)

	// session_start fires once per RunStream. Its AdditionalContext
	// (typically the AddEnvironmentInfo env block) is held as transient
	// extras and threaded into every model call below — never persisted,
	// to keep the visible transcript clean and the user message tail
	// stable.
	sessionStartMsgs := r.executeSessionStartHooks(ctx, sess, a, events)

	// Emit team information
	events <- TeamInfo(r.agentDetailsFromTeam(), a.Name())

	r.emitAgentWarnings(a, chanSend(events))
	r.configureToolsetHandlers(a, events)

	agentTools, err := r.getTools(ctx, a, sessionSpan, events, true)
	if err != nil {
		events <- Error(fmt.Sprintf("failed to get tools: %v", err))
		return
	}
	agentTools = filterExcludedTools(agentTools, sess.ExcludedTools)

	// Record the catalogue size on the session span — answers "how
	// many tools could this turn actually use?" without having to
	// walk into per-toolset spans. Stamped after exclusion filters
	// so the count matches what was offered to the model.
	sessionSpan.SetAttributes(attribute.Int("cagent.agent.tools.count", len(agentTools)))

	events <- ToolsetInfo(len(agentTools), false, a.Name())

	messages := sess.GetMessages(a)

	// Sub-sessions (transferred tasks, background agents, skill
	// sub-sessions) carry a synthesised "Please proceed." message that
	// no human authored. SendUserMessage is the same flag the runtime
	// uses to gate the UserMessageEvent, which is exactly the right
	// signal here too: "a real user prompt is at the tail of the session".
	var userPromptMsgs []chat.Message
	if sess.SendUserMessage && len(messages) > 0 {
		lastMsg := messages[len(messages)-1]
		events <- UserMessage(lastMsg.Content, sess.ID, lastMsg.MultiContent, len(sess.Messages)-1)

		// user_prompt_submit fires once per real user message, after
		// session_start and before the first model call.
		if lastMsg.Role == chat.MessageRoleUser {
			stop, msg, ctxMsgs := r.executeUserPromptSubmitHooks(ctx, sess, a, lastMsg.Content, events)
			if stop {
				slog.Warn("user_prompt_submit hook signalled run termination",
					"agent", a.Name(), "session_id", sess.ID, "reason", msg)
				r.emitHookDrivenShutdown(ctx, a, sess, msg, events)
				return
			}
			userPromptMsgs = ctxMsgs
		}
	}

	events <- StreamStarted(sess.ID, a.Name())

	defer r.finalizeEventChannel(ctx, sess, prevElicitationCh, events)

	// Response cache lookup. On a hit, replay the stored answer and
	// skip the model entirely. The matching storage half is
	// implemented as the cache_response stop-hook builtin (see
	// runtime/cache.go and getHooksExecutor).
	if r.tryReplayCachedResponse(ctx, sess, a, events) {
		return
	}

	iteration := 0
	// Use a runtime copy of maxIterations so we don't modify the session's persistent config
	runtimeMaxIterations := sess.MaxIterations

	// Initialize consecutive duplicate tool call detector
	//
	// Polling tools (view_background_agent, view_background_job) are
	// expected to be called repeatedly with identical arguments while a
	// background task is in progress. Exempt them so they never trigger
	// the loop-termination path.
	loopThreshold := sess.MaxConsecutiveToolCalls
	if loopThreshold == 0 {
		loopThreshold = 5 // default: always active
	}
	loopDetector := toolexec.NewLoopDetector(loopThreshold,
		bgagent.ToolNameViewBackgroundAgent,
		builtin.ToolNameViewBackgroundJob,
	)

	// overflowCompactions counts how many consecutive context-overflow
	// auto-compactions have been attempted without a successful model
	// call in between. The cap (r.maxOverflowCompactions) prevents an
	// infinite loop when compaction cannot reduce the context below the
	// model's limit; see defaultMaxOverflowCompactions for the default
	// and WithMaxOverflowCompactions for the test seam.
	var overflowCompactions int

	// toolModelOverride holds the per-toolset model from the most recent
	// tool calls. It applies for one LLM turn, then resets.
	var toolModelOverride string
	var prevAgentName string

	for {
		// Pause the loop here if /pause has been toggled on. Any in-flight
		// LLM request and its tool calls have already completed.
		if err := r.waitIfPaused(ctx); err != nil {
			return
		}

		a = r.resolveSessionAgent(sess)

		// Clear per-tool model override on agent switch so it doesn't
		// leak from one agent's toolset into another agent's turn.
		if a.Name() != prevAgentName {
			toolModelOverride = ""
			prevAgentName = a.Name()
		}

		r.emitAgentWarnings(a, chanSend(events))
		r.configureToolsetHandlers(a, events)

		agentTools, err := r.getTools(ctx, a, sessionSpan, events, true)
		if err != nil {
			events <- Error(fmt.Sprintf("failed to get tools: %v", err))
			return
		}
		agentTools = filterExcludedTools(agentTools, sess.ExcludedTools)

		// Emit updated tool count. After a ToolListChanged MCP notification
		// the cache is invalidated, so getTools above re-fetches from the
		// server and may return a different count.
		events <- ToolsetInfo(len(agentTools), false, a.Name())

		// Check iteration limit
		newMax, decision := r.enforceMaxIterations(ctx, sess, a, iteration, runtimeMaxIterations, events)
		if decision == iterationStop {
			return
		}
		runtimeMaxIterations = newMax

		iteration++

		// Exit immediately if the stream context has been cancelled (e.g., Ctrl+C)
		if err := ctx.Err(); err != nil {
			slog.Debug("Runtime stream context cancelled, stopping loop", "agent", a.Name(), "session_id", sess.ID)
			return
		}
		slog.Debug("Starting conversation loop iteration", "agent", a.Name())

		model := a.Model()

		// Per-tool model routing: use a cheaper model for this turn
		// if the previous tool calls specified one, then reset.
		if toolModelOverride != "" {
			if overrideModel, err := r.resolveModelRef(ctx, toolModelOverride); err != nil {
				slog.Warn("Failed to resolve per-tool model override; using agent default",
					"model_override", toolModelOverride, "error", err)
			} else {
				slog.Info("Using per-tool model override for this turn",
					"agent", a.Name(), "override", overrideModel.ID(), "primary", model.ID())
				model = overrideModel
			}
			toolModelOverride = ""
		}

		modelID := model.ID()

		// Notify sidebar of the model for this turn. For rule-based
		// routing, the actual routed model is emitted from within the
		// stream once the first chunk arrives.
		events <- AgentInfo(a.Name(), modelID, a.Description(), a.WelcomeMessage())

		slog.Debug("Using agent", "agent", a.Name(), "model", modelID)
		slog.Debug("Getting model definition", "model_id", modelID)
		m, err := r.modelsStore.GetModel(ctx, modelID)
		if err != nil {
			slog.Debug("Failed to get model definition", "error", err)
		}
		// We can only compact if we know the limit.
		var contextLimit int64
		if m != nil {
			contextLimit = int64(m.Limit.Context)

			if r.sessionCompaction && compaction.ShouldCompact(sess.InputTokens, sess.OutputTokens, 0, contextLimit) {
				r.compactWithReason(ctx, sess, "", compactionReasonThreshold, events)
			}
		}

		// Drain steer messages queued while idle or before the first model call
		// (covers idle-window and first-turn-miss races).
		if drained, messageCountBeforeSteer := r.drainAndEmitSteered(ctx, sess, events); drained {
			r.compactIfNeeded(ctx, sess, a, m, contextLimit, messageCountBeforeSteer, events)
		}

		// Everything from turn_start onwards is wrapped in a closure so a
		// single deferred turn_end hook fires on every exit path: a normal
		// stop, a follow-up continue, an error, a hook-driven shutdown, the
		// loop-detector tripping, ctx cancellation, even a panic. The
		// closure returns the loop control directive and the reason string
		// reported via [hooks.Input.Reason]; the deferred dispatch then runs
		// AFTER the closure body has assigned both, so callers see the same
		// reason the runtime took. ctrl drives the outer for-loop's
		// continue-or-exit decision.
		ctrl := r.runTurn(ctx, sess, a, m, model, modelID, contextLimit, sessionSpan,
			slices.Concat(sessionStartMsgs, userPromptMsgs),
			agentTools, loopDetector, &overflowCompactions, &toolModelOverride, events)
		switch ctrl {
		case turnContinue:
			continue
		case turnExit:
			return
		}
	}
}

// turnControl is what [LocalRuntime.runTurn] reports back to the outer
// run-stream loop: continue to the next iteration, or exit the loop
// entirely. break and return are equivalent here because the loop is
// the last statement in runStreamLoop, so we collapse them into one.
type turnControl int

const (
	// turnContinue — outer loop should re-iterate (e.g. follow-up,
	// drained steered, retry after stream error, more tool calls).
	turnContinue turnControl = iota
	// turnExit — outer loop should stop and let runStreamLoop’s
	// deferred cleanup run (normal stop, error, hook-blocked,
	// loop-detected, ctx cancelled).
	turnExit
)

// runTurn performs one iteration of the run-stream loop, from
// turn_start onwards. Wrapping the body in its own function exists for
// one reason: a deferred call can fire turn_end on every exit path — a
// normal stop, an error from handleStreamError, a hook-driven
// shutdown, the loop detector, context cancellation, even a panic —
// without sprinkling explicit dispatch calls at every return / break /
// continue. endReason is captured by reference so each branch can set
// it before falling out; the deferred call reads it AFTER the body has
// assigned the final value.
//
// The outer loop owns persistent per-stream state (iteration counter,
// session-start extras, agent-switch tracking); per-turn state that
// needs to survive into the next iteration (overflowCompactions,
// toolModelOverride) is passed by pointer so this function can mutate
// it the same way the inline body did.
func (r *LocalRuntime) runTurn(
	ctx context.Context,
	sess *session.Session,
	a *agent.Agent,
	m *modelsdev.Model,
	model provider.Provider,
	modelID string,
	contextLimit int64,
	sessionSpan trace.Span,
	priorExtras []chat.Message,
	agentTools []tools.Tool,
	loopDetector *toolexec.LoopDetector,
	overflowCompactions *int,
	toolModelOverride *string,
	events chan Event,
) (ctrl turnControl) {
	streamAttrs := []attribute.KeyValue{
		attribute.String(genai.AttrConversationID, sess.ID),
		attribute.String(genai.AttrAgentNameRuntime, a.Name()),
	}
	if genai.EmitLegacyAttributes() {
		streamAttrs = append(streamAttrs,
			attribute.String("agent", a.Name()),
			attribute.String("session.id", sess.ID),
		)
	}
	streamCtx, streamSpan := r.startSpan(ctx, "runtime.stream", trace.WithAttributes(streamAttrs...))
	// streamSpan ends inline at the natural points (success path before
	// recordAssistantMessage, error path after handleStreamError) so its
	// duration tracks the model call only, not the whole iteration. The
	// boolean prevents a double-End on paths that already closed it.
	spanEnded := false
	endStreamSpan := func() {
		if !spanEnded {
			streamSpan.End()
			spanEnded = true
		}
	}
	defer endStreamSpan()

	// endReason is set by every exit branch below and read by the
	// deferred turn_end dispatch. Default = normal so a clean fall-
	// through (model produced output, more tool calls, no hook
	// blocked) reports "continue" or "normal" depending on which
	// branch ran last. Branches overwrite this before returning.
	endReason := turnEndReasonNormal
	defer func() {
		if ctxErr := ctx.Err(); ctxErr != nil && endReason == turnEndReasonNormal {
			// Context cancellation is detected after the fact: a
			// branch that returned early because of ctx.Err overrides
			// the default, but a panic-recovered branch may not have
			// had the chance, so re-check here.
			endReason = turnEndReasonCanceled
		}
		// Use a non-cancellable context so turn_end runs even when
		// the stream was interrupted (Ctrl+C, parent cancellation),
		// matching the same guarantee session_end has at the
		// finalizeEventChannel level.
		r.executeTurnEndHooks(context.WithoutCancel(ctx), sess, a, endReason, events)
	}()

	// Run turn_start hooks BEFORE building messages so their
	// AdditionalContext, alongside the session_start extras captured
	// once at the top of RunStream, can be spliced after the invariant
	// cache checkpoint and before the conversation history. Neither
	// hook's output is persisted, so per-turn signals (date, prompt
	// files) refresh every turn while session-level context (cwd, OS,
	// arch) stays stable — all without bloating the stored history.
	turnStartMsgs := r.executeTurnStartHooks(ctx, sess, a, events)
	messages := sess.GetMessages(a, slices.Concat(priorExtras, turnStartMsgs)...)
	slog.Debug("Retrieved messages for processing", "agent", a.Name(), "message_count", len(messages))

	// before_llm_call hooks fire just before the model is invoked.
	// A terminating verdict (e.g. from the max_iterations builtin)
	// stops the run loop here, before any tokens are spent. Hooks
	// may also rewrite the outgoing messages by returning
	// HookSpecificOutput.UpdatedMessages — the redact_secrets
	// builtin uses this to scrub secrets from chat content before
	// the LLM ever sees it. The rewrite happens BEFORE the
	// runtime's Go-only message transforms so a hook that drops a
	// message (e.g. a custom "strip system reminders") doesn't get
	// silently overridden by a transform later in the chain.
	stop, msg, rewritten := r.executeBeforeLLMCallHooks(ctx, sess, a, modelID, messages)
	if stop {
		slog.Warn("before_llm_call hook signalled run termination",
			"agent", a.Name(), "session_id", sess.ID, "reason", msg)
		r.emitHookDrivenShutdown(ctx, a, sess, msg, events)
		endStreamSpan()
		endReason = turnEndReasonHookBlocked
		return turnExit
	}
	if rewritten != nil {
		messages = rewritten
	}

	// Apply registered before_llm_call message transforms (e.g.
	// strip_unsupported_modalities for text-only models, plus any
	// embedder-supplied redactor / scrubber registered via
	// WithMessageTransform). Runs after the gate so a transform
	// failure cannot waste the gate's allow verdict. modelID is
	// passed explicitly so transforms see the actual model the
	// loop chose (per-tool override + alloy-mode selection),
	// not whatever a fresh agent.Model() call would re-randomize.
	messages = r.applyBeforeLLMCallTransforms(ctx, sess, a, modelID, messages)

	// Try primary model with fallback chain if configured
	res, usedModel, err := r.fallback.execute(streamCtx, a, model, messages, agentTools, sess, m, events)
	if err != nil {
		outcome := r.handleStreamError(ctx, sess, a, err, contextLimit, overflowCompactions, streamSpan, events)
		endStreamSpan()
		endReason = turnEndReasonError
		if outcome == streamErrorRetry {
			return turnContinue
		}
		return turnExit
	}

	// A successful model call resets the overflow compaction counter.
	*overflowCompactions = 0

	// after_llm_call hooks fire on success only; failed calls
	// fire on_error above. The assistant text content is passed
	// via stop_response, matching the stop event's payload, so
	// handlers can reuse the same parsing.
	r.executeAfterLLMCallHooks(ctx, sess, a, res.Content)

	if usedModel != nil && usedModel.ID() != model.ID() {
		slog.Info("Used fallback model", "agent", a.Name(), "primary", model.ID(), "used", usedModel.ID())
		events <- AgentInfo(a.Name(), usedModel.ID(), a.Description(), a.WelcomeMessage())
	}
	streamSpan.SetAttributes(
		attribute.Int("tool.calls", len(res.Calls)),
		attribute.Int("content.length", len(res.Content)),
		attribute.Bool("stopped", res.Stopped),
	)
	endStreamSpan()
	slog.Debug("Stream processed", "agent", a.Name(), "tool_calls", len(res.Calls), "content_length", len(res.Content), "stopped", res.Stopped)

	msgUsage := r.recordAssistantMessage(sess, a, res, agentTools, modelID, m, events)

	usage := SessionUsage(sess, contextLimit)
	usage.LastMessage = msgUsage
	events <- NewTokenUsageEvent(sess.ID, a.Name(), usage)

	// Record the message count before tool calls so we can
	// measure how much content was added by tool results.
	messageCountBeforeTools := len(sess.GetAllMessages())

	stopRun, stopMsg := r.processToolCalls(ctx, sess, res.Calls, agentTools, events)

	// Re-probe toolsets after tool calls: an install/setup tool call may
	// have made a previously-unavailable LSP or MCP connectable. reprobe()
	// calls ensureToolSetsAreStarted, emits recovery notices, and updates
	// the TUI tool-count immediately.
	//
	// The new tools are picked up by the next iteration's getTools() call
	// at the top of this loop, so the model sees them on its very next
	// response — within the same user turn, without requiring a new user
	// message. reprobe's return value is intentionally discarded here;
	// the top-of-loop getTools() is the authoritative source.
	if len(res.Calls) > 0 {
		r.reprobe(ctx, sess, a, agentTools, sessionSpan, events)
	}

	// Check for degenerate tool call loops
	if loopDetector.Record(res.Calls) {
		toolName := "unknown"
		if len(res.Calls) > 0 {
			toolName = res.Calls[0].Function.Name
		}
		consecutive := loopDetector.Consecutive()
		slog.Warn("Repetitive tool call loop detected",
			"agent", a.Name(), "tool", toolName,
			"consecutive", consecutive, "session_id", sess.ID)
		errMsg := fmt.Sprintf(
			"Agent terminated: detected %d consecutive identical calls to %s. "+
				"This indicates a degenerate loop where the model is not making progress.",
			consecutive, toolName)
		// Mark the session span as Error so loop-termination shows up
		// in trace status / error-rate dashboards instead of blending
		// in with normal completions.
		sessionSpan.SetAttributes(
			attribute.String("error.type", "loop_detected"),
			attribute.String("cagent.session.terminated_by", "loop_detector"),
			attribute.Int("cagent.loop.consecutive_calls", consecutive),
		)
		sessionSpan.SetStatus(codes.Error, errMsg)
		events <- Error(errMsg)
		r.notifyError(ctx, a, sess.ID, errMsg)
		loopDetector.Reset()
		endReason = turnEndReasonLoopDetected
		return turnExit
	}

	// post_tool_use hook signalled run termination via a deny
	// verdict (decision="block" / continue=false / exit 2).
	// User-authored hooks can use this to stop the run; the
	// runtime fans out the standard Error / notification /
	// on_error stanzas before exiting.
	if stopRun {
		slog.Warn("post_tool_use hook signalled run termination",
			"agent", a.Name(), "session_id", sess.ID, "reason", stopMsg)
		r.emitHookDrivenShutdown(ctx, a, sess, stopMsg, events)
		endReason = turnEndReasonHookBlocked
		return turnExit
	}

	// Record per-toolset model override for the next LLM turn.
	*toolModelOverride = toolexec.ResolveModelOverride(res.Calls, agentTools)

	// Drain steer messages that arrived during tool calls.
	if drained, _ := r.drainAndEmitSteered(ctx, sess, events); drained {
		r.compactIfNeeded(ctx, sess, a, m, contextLimit, messageCountBeforeTools, events)
		endReason = turnEndReasonSteered
		return turnContinue
	}

	if res.Stopped {
		slog.Debug("Conversation stopped", "agent", a.Name())
		r.executeStopHooks(ctx, sess, a, res.Content, events)

		// Re-check steer queue: closes the race between the mid-loop drain and this stop.
		if drained, _ := r.drainAndEmitSteered(ctx, sess, events); drained {
			r.compactIfNeeded(ctx, sess, a, m, contextLimit, messageCountBeforeTools, events)
			endReason = turnEndReasonSteered
			return turnContinue
		}

		// --- FOLLOW-UP: end-of-turn injection ---
		// Pop exactly one follow-up message. Unlike steered
		// messages, follow-ups are plain user messages that start
		// a new turn — the model sees them as fresh input, not a
		// mid-stream interruption. Each follow-up gets a full
		// undivided agent turn.
		if followUp, ok := r.followUpQueue.Dequeue(ctx); ok {
			userMsg := session.UserMessage(followUp.Content, followUp.MultiContent...)
			sess.AddMessage(userMsg)
			events <- UserMessage(followUp.Content, sess.ID, followUp.MultiContent, len(sess.Messages)-1)
			r.compactIfNeeded(ctx, sess, a, m, contextLimit, messageCountBeforeTools, events)
			endReason = turnEndReasonContinue
			return turnContinue // re-enter the loop for a new turn
		}

		endReason = turnEndReasonNormal
		return turnExit
	}

	r.compactIfNeeded(ctx, sess, a, m, contextLimit, messageCountBeforeTools, events)
	endReason = turnEndReasonContinue
	return turnContinue
}

// Run executes the agent loop synchronously and returns the final session
// messages. This is a convenience wrapper around RunStream for non-streaming
// callers.
func (r *LocalRuntime) Run(ctx context.Context, sess *session.Session) ([]session.Message, error) {
	events := r.RunStream(ctx, sess)
	for event := range events {
		if errEvent, ok := event.(*ErrorEvent); ok {
			return nil, fmt.Errorf("%s", errEvent.Error)
		}
	}
	return sess.GetAllMessages(), nil
}

// recordAssistantMessage adds the model's response to the session and returns
// per-message usage information for the token-usage event. Empty responses
// (no text and no tool calls) are silently skipped since providers reject them.
func (r *LocalRuntime) recordAssistantMessage(
	sess *session.Session,
	a *agent.Agent,
	res streamResult,
	agentTools []tools.Tool,
	modelID string,
	m *modelsdev.Model,
	events chan Event,
) *MessageUsage {
	if strings.TrimSpace(res.Content) == "" && len(res.Calls) == 0 {
		slog.Debug("Skipping empty assistant message (no content and no tool calls)", "agent", a.Name())
		return nil
	}

	// Resolve tool definitions for the tool calls.
	var toolDefs []tools.Tool
	if len(res.Calls) > 0 {
		toolMap := make(map[string]tools.Tool, len(agentTools))
		for _, t := range agentTools {
			toolMap[t.Name] = t
		}
		for _, call := range res.Calls {
			if def, ok := toolMap[call.Function.Name]; ok {
				toolDefs = append(toolDefs, def)
			}
		}
	}

	// Calculate per-message cost when pricing information is available.
	var messageCost float64
	if res.Usage != nil && m != nil && m.Cost != nil {
		messageCost = (float64(res.Usage.InputTokens)*m.Cost.Input +
			float64(res.Usage.OutputTokens)*m.Cost.Output +
			float64(res.Usage.CachedInputTokens)*m.Cost.CacheRead +
			float64(res.Usage.CacheWriteTokens)*m.Cost.CacheWrite) / 1e6
	}

	messageModel := modelID

	assistantMessage := chat.Message{
		Role:              chat.MessageRoleAssistant,
		Content:           res.Content,
		ReasoningContent:  res.ReasoningContent,
		ThinkingSignature: res.ThinkingSignature,
		ThoughtSignature:  res.ThoughtSignature,
		ToolCalls:         res.Calls,
		ToolDefinitions:   toolDefs,
		CreatedAt:         r.now().Format(time.RFC3339),
		Usage:             res.Usage,
		Model:             messageModel,
		Cost:              messageCost,
		FinishReason:      res.FinishReason,
	}

	addAgentMessage(sess, a, &assistantMessage, events)
	slog.Debug("Added assistant message to session", "agent", a.Name(), "total_messages", len(sess.GetAllMessages()))

	// Build per-message usage for the event.
	if res.Usage == nil {
		return nil
	}
	msgUsage := &MessageUsage{
		Usage:        *res.Usage,
		Cost:         messageCost,
		Model:        messageModel,
		FinishReason: res.FinishReason,
	}
	return msgUsage
}

// compactIfNeeded estimates the token impact of tool results added since
// messageCountBefore and triggers proactive compaction when the estimated
// total exceeds 90% of the context window. This prevents sending an
// oversized request on the next iteration.
func (r *LocalRuntime) compactIfNeeded(
	ctx context.Context,
	sess *session.Session,
	a *agent.Agent,
	m *modelsdev.Model,
	contextLimit int64,
	messageCountBefore int,
	events chan Event,
) {
	if m == nil || !r.sessionCompaction || contextLimit <= 0 {
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

	slog.Info("Proactive compaction: tool results pushed estimated context past 90%% threshold",
		"agent", a.Name(),
		"input_tokens", sess.InputTokens,
		"output_tokens", sess.OutputTokens,
		"added_estimated_tokens", addedTokens,
		"estimated_total", sess.InputTokens+sess.OutputTokens+addedTokens,
		"context_limit", contextLimit,
	)
	r.compactWithReason(ctx, sess, "", compactionReasonThreshold, events)
}

// getTools executes tool retrieval with automatic OAuth handling.
// emitLifecycleEvents controls whether MCPInitStarted/Finished are emitted;
// pass false when calling from reprobe to avoid spurious TUI spinner flicker.
func (r *LocalRuntime) getTools(ctx context.Context, a *agent.Agent, sessionSpan trace.Span, events chan Event, emitLifecycleEvents bool) ([]tools.Tool, error) {
	if emitLifecycleEvents && len(a.ToolSets()) > 0 {
		events <- MCPInitStarted(a.Name())
		defer func() { events <- MCPInitFinished(a.Name()) }()
	}

	agentTools, err := a.Tools(ctx)
	if err != nil {
		slog.Error("Failed to get agent tools", "agent", a.Name(), "error", err)
		sessionSpan.RecordError(err)
		sessionSpan.SetStatus(codes.Error, "failed to get tools")
		r.telemetry.RecordError(ctx, err.Error())
		return nil, err
	}

	slog.Debug("Retrieved agent tools", "agent", a.Name(), "tool_count", len(agentTools))
	return agentTools, nil
}

// configureToolsetHandlers sets up elicitation and OAuth handlers for all toolsets of an agent.
func (r *LocalRuntime) configureToolsetHandlers(a *agent.Agent, events chan Event) {
	for _, toolset := range a.ToolSets() {
		tools.ConfigureHandlers(toolset,
			r.elicitationHandler,
			func() { events <- Authorization(tools.ElicitationActionAccept, a.Name()) },
			r.managedOAuth,
		)

		// Wire RAG event forwarding so the TUI shows indexing progress.
		if ragTool, ok := tools.As[*builtin.RAGTool](toolset); ok {
			ragTool.SetEventCallback(ragEventForwarder(ragTool.Name(), r, chanSend(events)))
		}
	}
}

// emitAgentWarnings drains and emits any pending toolset warnings as
// persistent TUI notifications. Failures ("start failed", "list failed")
// are surfaced so the user can act on them; recoveries are intentionally
// not emitted — "X is now available" reads as a spurious warning right
// after the user completes an OAuth dance, and adds no signal for other
// recoveries either.
func (r *LocalRuntime) emitAgentWarnings(a *agent.Agent, send func(Event)) {
	warnings := a.DrainWarnings()
	if len(warnings) == 0 {
		return
	}
	slog.Warn("Tool setup partially failed; continuing", "agent", a.Name(), "warnings", warnings)
	send(Warning(formatToolWarning(a, warnings), a.Name()))
}

func formatToolWarning(a *agent.Agent, warnings []string) string {
	var builder strings.Builder
	fmt.Fprintf(&builder, "Some toolsets failed to initialize for agent '%s'.\n\nDetails:\n\n", a.Name())
	for _, warning := range warnings {
		fmt.Fprintf(&builder, "- %s\n", warning)
	}
	return strings.TrimSuffix(builder.String(), "\n")
}

// filterExcludedTools removes tools whose names appear in the excluded list.
// This is used by skill sub-sessions to prevent recursive run_skill calls.
func filterExcludedTools(agentTools []tools.Tool, excluded []string) []tools.Tool {
	if len(excluded) == 0 {
		return agentTools
	}
	excludeSet := make(map[string]bool, len(excluded))
	for _, name := range excluded {
		excludeSet[name] = true
	}
	filtered := make([]tools.Tool, 0, len(agentTools))
	for _, t := range agentTools {
		if !excludeSet[t.Name] {
			filtered = append(filtered, t)
		}
	}
	return filtered
}

// chanSend wraps a channel as a func(Event) for use with emitAgentWarnings
// and RAG event forwarding. The send is non-blocking: if the channel is full
// or closed, the event is silently dropped. This prevents a panic when a
// long-lived goroutine (e.g. RAG file watcher) tries to forward an event
// after the per-message events channel has been closed.
func chanSend(ch chan Event) func(Event) {
	return func(e Event) {
		defer func() { recover() }() //nolint:errcheck // swallow send-on-closed-channel panic
		select {
		case ch <- e:
		default:
		}
	}
}

// reprobe re-runs ensureToolSetsAreStarted after a batch of tool calls.
// If new tools became available (by name-set diff), it emits a ToolsetInfo
// event to update the TUI immediately. The new tools will be picked up by
// the next iteration's getTools() call at the top of the loop.
//
// reprobe deliberately does NOT return the new tool list: the top-of-loop
// getTools() is the single authoritative source for agentTools each iteration.
func (r *LocalRuntime) reprobe(
	ctx context.Context,
	sess *session.Session,
	a *agent.Agent,
	currentTools []tools.Tool,
	sessionSpan trace.Span,
	events chan Event,
) {
	updated, err := r.getTools(ctx, a, sessionSpan, events, false)
	if err != nil {
		slog.Warn("reprobe: getTools failed", "agent", a.Name(), "error", err)
		return
	}
	updated = filterExcludedTools(updated, sess.ExcludedTools)

	// Emit any pending warnings that getTools just generated.
	r.emitAgentWarnings(a, chanSend(events))

	// Compute added tools by comparing name-sets (not just counts), so we
	// correctly handle a toolset that replaced one tool with another.
	prev := make(map[string]struct{}, len(currentTools))
	for _, t := range currentTools {
		prev[t.Name] = struct{}{}
	}
	var added []string
	for _, t := range updated {
		if _, exists := prev[t.Name]; !exists {
			added = append(added, t.Name)
		}
	}

	if len(added) == 0 {
		return
	}

	slog.Info("New tools available after toolset re-probe",
		"agent", a.Name(), "added", added)

	// Emit updated tool count to the TUI immediately.
	chanSend(events)(ToolsetInfo(len(updated), false, a.Name()))
}
