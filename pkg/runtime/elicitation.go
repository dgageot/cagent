package runtime

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"sync"

	"github.com/modelcontextprotocol/go-sdk/mcp"

	"github.com/docker/docker-agent/pkg/tools"
)

// ElicitationResult represents the result of an elicitation request.
//
// Returned by the embedder via ResumeElicitation when the user responds to a
// schema-driven prompt that an MCP server (or the runtime) requested.
type ElicitationResult struct {
	Action  tools.ElicitationAction
	Content map[string]any // The submitted form data (only present when action is "accept")
}

// ElicitationError represents a declined or cancelled elicitation, exposed
// to callers that prefer error-style propagation over an Action value.
type ElicitationError struct {
	Action  string
	Message string
}

func (e *ElicitationError) Error() string {
	return fmt.Sprintf("elicitation %s: %s", e.Action, e.Message)
}

// ElicitationRequestHandler is the callback signature an embedder can supply
// to handle inbound elicitation requests directly (e.g. an HTTP server).
type ElicitationRequestHandler func(ctx context.Context, message string, schema map[string]any) (map[string]any, error)

// errNoElicitationChannel is returned when the bridge has no channel
// configured (no RunStream is active).
var errNoElicitationChannel = errors.New("no events channel available for elicitation")

// elicitationBridge owns the events channel that the runtime's MCP
// elicitation handler sends requests to. Each RunStream call swaps in its
// own channel on entry and the previous one back on exit, so nested
// sub-session streams don't lose the parent's elicitation pipe.
//
// The bridge encapsulates a non-trivial concurrency contract: while a
// caller holds a reference to the current channel and is in the middle
// of sending an elicitation request, stream teardown must not race with
// close(channel) on the inner stream. We achieve this by serializing
// send, swap, and close with an RWMutex held across the channel
// operation. Pushing this into a small standalone type keeps the
// contract testable in isolation (with the race detector) without
// spinning up a runtime, and keeps LocalRuntime free of the two raw
// fields it used to expose.
type elicitationBridge struct {
	mu sync.RWMutex
	ch chan Event
}

// swap atomically replaces the bridge's channel and returns the previous
// value. RunStream calls swap(events) on entry and swap(prev) on exit.
func (b *elicitationBridge) swap(ch chan Event) chan Event {
	b.mu.Lock()
	defer b.mu.Unlock()
	prev := b.ch
	b.ch = ch
	return prev
}

// send delivers ev to the current channel, holding the read lock across
// the send. This blocks concurrent teardown until the send completes,
// preserving the invariant that the channel reference held by an
// in-flight sender stays open until that sender finishes.
//
// Returns errNoElicitationChannel when no channel is configured or when
// a defensive recover catches an externally closed channel.
func (b *elicitationBridge) send(ev Event) (err error) {
	b.mu.RLock()
	defer b.mu.RUnlock()
	defer func() {
		if recover() != nil {
			err = errNoElicitationChannel
		}
	}()
	if b.ch == nil {
		return errNoElicitationChannel
	}
	b.ch <- ev
	return nil
}

// restoreAndClose restores the previous stream channel and closes the current
// stream channel under the bridge write lock, so the close is mutually
// exclusive with an in-flight send. This is the #3069 fix: close can no longer
// race a parked sender and panic with "send on closed channel".
//
// Accepted trade-off (do not "fix" by dropping the lock): holding the write
// lock makes restoreAndClose wait for any in-flight send to finish, because
// send holds the read lock across "b.ch <- ev". If the stream consumer has
// gone away and current is full (or unbuffered), that parked send never
// drains, so this call blocks on Lock forever and the teardown goroutine
// leaks. A leaked goroutine is the deliberate, accepted alternative to
// crashing the whole process with a send-on-closed-channel panic.
func (b *elicitationBridge) restoreAndClose(current, previous chan Event) {
	b.mu.Lock()
	defer b.mu.Unlock()
	b.ch = previous
	close(current)
}

// ResumeElicitation sends an elicitation response back to a waiting
// elicitation request. Returns an error if no elicitation is in progress
// or if the context is cancelled before the response can be delivered.
func (r *LocalRuntime) ResumeElicitation(ctx context.Context, action tools.ElicitationAction, content map[string]any) error {
	slog.DebugContext(ctx, "Resuming runtime with elicitation response", "agent", r.currentAgentName(), "action", action)

	result := ElicitationResult{
		Action:  action,
		Content: content,
	}

	select {
	case <-ctx.Done():
		slog.DebugContext(ctx, "Context cancelled while sending elicitation response")
		return ctx.Err()
	case r.elicitationRequestCh <- result:
		slog.DebugContext(ctx, "Elicitation response sent successfully", "action", action)
		return nil
	default:
		slog.DebugContext(ctx, "Elicitation channel not ready")
		return errors.New("no elicitation request in progress")
	}
}

// elicitationHandler is the MCP-toolset-side hook that turns an inbound
// elicitation request from a server into an ElicitationRequest event on the
// active stream's events channel and waits for the embedder's response on
// elicitationRequestCh.
func (r *LocalRuntime) elicitationHandler(ctx context.Context, req *mcp.ElicitParams) (tools.ElicitationResult, error) {
	slog.DebugContext(ctx, "Elicitation request received from MCP server", "message", req.Message)

	// In non-interactive mode (e.g., MCP serve), there is no user to respond
	// to elicitation requests. Decline immediately instead of blocking forever.
	if r.nonInteractive {
		slog.DebugContext(ctx, "Declining elicitation in non-interactive mode", "message", req.Message)
		return tools.ElicitationResult{
			Action: tools.ElicitationActionDecline,
		}, nil
	}

	r.executeOnUserInputHooks(ctx, "", "elicitation")

	slog.DebugContext(ctx, "Sending elicitation request event to client",
		"message", req.Message,
		"mode", req.Mode,
		"requested_schema", req.RequestedSchema,
		"url", req.URL)
	slog.DebugContext(ctx, "Elicitation request meta", "meta", req.Meta)

	if err := r.elicitation.send(
		ElicitationRequest(req.Message, req.Mode, req.RequestedSchema, req.URL, req.ElicitationID, req.Meta, r.currentAgentName()),
	); err != nil {
		return tools.ElicitationResult{}, err
	}

	// Wait for response from the client.
	select {
	case result := <-r.elicitationRequestCh:
		return tools.ElicitationResult{
			Action:  result.Action,
			Content: result.Content,
		}, nil
	case <-ctx.Done():
		slog.DebugContext(ctx, "Context cancelled while waiting for elicitation response")
		return tools.ElicitationResult{}, ctx.Err()
	}
}
