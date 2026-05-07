package genai

import (
	"errors"
	"io"
	"strings"
	"sync"

	"github.com/docker/docker-agent/pkg/chat"
	"github.com/docker/docker-agent/pkg/tools"
)

// StreamAttributer is an optional interface providers may implement to
// surface provider-specific attributes to the chat span on Close.
type StreamAttributer interface {
	GenAIStreamAttributes() []KeyValue
}

// KeyValue is an attribute key/value pair used by StreamAttributer so
// providers don't need to import otel/attribute.
type KeyValue struct {
	Key   string
	Value any
}

// WrapStream wraps a chat.MessageStream so that consuming the stream drives
// the lifecycle of a ChatSpan: per-chunk timing, response-level attributes,
// usage capture, and span End on close or terminal error.
func WrapStream(span *ChatSpan, stream chat.MessageStream) chat.MessageStream {
	if span == nil || stream == nil {
		return stream
	}
	return &instrumentedStream{
		span:    span,
		inner:   stream,
		capture: IsContentCaptureEnabled(),
	}
}

type instrumentedStream struct {
	span  *ChatSpan
	inner chat.MessageStream

	// mu guards lifecycle flags and the streaming-state buffers.
	mu sync.Mutex

	// ended is set when the span has been finalised. innerClosed is set
	// when the inner stream's Close has been called.
	ended       bool
	innerClosed bool

	// capture buffers the streamed deltas for emission as gen_ai.output.messages.
	capture       bool
	contentBuf    strings.Builder
	reasoningBuf  strings.Builder
	pendingTools  map[string]*tools.ToolCall
	toolCallOrder []string
}

func (s *instrumentedStream) Recv() (chat.MessageStreamResponse, error) {
	resp, err := s.inner.Recv()
	if err != nil {
		// io.EOF is the normal stream terminator; non-EOF errors end the
		// span here so the duration metric is not lost when the caller
		// abandons the stream.
		if !errors.Is(err, io.EOF) {
			s.span.RecordError(err, ClassifyError(err))
			s.endOnce()
		}
		return resp, err
	}

	// First-chunk arrival drives time_to_first_chunk; only count
	// chunks with payload to ignore empty preambles.
	if hasChunkPayload(&resp) {
		s.span.MarkChunk()
	}

	if resp.ID != "" {
		s.span.SetResponseID(resp.ID)
	}
	if resp.Model != "" {
		s.span.SetResponseModel(resp.Model)
	}
	for i := range resp.Choices {
		if resp.Choices[i].FinishReason != "" {
			s.span.AddFinishReason(string(resp.Choices[i].FinishReason))
		}
	}
	if resp.Usage != nil {
		s.span.RecordUsage(
			resp.Usage.InputTokens,
			resp.Usage.OutputTokens,
			resp.Usage.CachedInputTokens,
			resp.Usage.CacheWriteTokens,
			resp.Usage.ReasoningTokens,
		)
	}

	if s.capture {
		s.mu.Lock()
		s.bufferDeltas(&resp)
		s.mu.Unlock()
	}
	return resp, nil
}

// bufferDeltas accumulates content and tool-call deltas. Tool calls arrive
// across multiple chunks (id once, name once, arguments in pieces).
func (s *instrumentedStream) bufferDeltas(resp *chat.MessageStreamResponse) {
	for i := range resp.Choices {
		d := &resp.Choices[i].Delta
		if d.Content != "" {
			s.contentBuf.WriteString(d.Content)
		}
		if d.ReasoningContent != "" {
			s.reasoningBuf.WriteString(d.ReasoningContent)
		}
		for j := range d.ToolCalls {
			tc := &d.ToolCalls[j]
			id := tc.ID
			if id == "" {
				// Fall back to the most recent in-progress tool call.
				if len(s.toolCallOrder) == 0 {
					continue
				}
				id = s.toolCallOrder[len(s.toolCallOrder)-1]
			}
			if s.pendingTools == nil {
				s.pendingTools = map[string]*tools.ToolCall{}
			}
			existing, ok := s.pendingTools[id]
			if !ok {
				existing = &tools.ToolCall{ID: id, Type: tc.Type}
				s.pendingTools[id] = existing
				s.toolCallOrder = append(s.toolCallOrder, id)
			}
			if tc.Function.Name != "" {
				existing.Function.Name = tc.Function.Name
			}
			if tc.Function.Arguments != "" {
				existing.Function.Arguments += tc.Function.Arguments
			}
		}
	}
}

func (s *instrumentedStream) Close() {
	s.mu.Lock()
	closeInner := !s.innerClosed
	s.innerClosed = true
	s.mu.Unlock()
	if closeInner {
		s.inner.Close()
	}
	s.endOnce()
}

// endOnce flushes captured content and ends the span at most once. Both the
// Recv error path and explicit Close go through here. inner.Close is NOT
// called here — only the explicit Close path releases the inner stream.
func (s *instrumentedStream) endOnce() {
	s.mu.Lock()
	if s.ended {
		s.mu.Unlock()
		return
	}
	s.ended = true
	// Snapshot under lock and release before calling out to OTel SDK.
	var (
		extras       []KeyValue
		captured     bool
		content      string
		reasoning    string
		collected    []tools.ToolCall
		streamAttrer StreamAttributer
	)
	if attrer, ok := s.inner.(StreamAttributer); ok {
		streamAttrer = attrer
	}
	if s.capture {
		captured = true
		content = s.contentBuf.String()
		reasoning = s.reasoningBuf.String()
		for _, id := range s.toolCallOrder {
			if tc, ok := s.pendingTools[id]; ok {
				collected = append(collected, *tc)
			}
		}
	}
	s.mu.Unlock()

	if streamAttrer != nil {
		extras = streamAttrer.GenAIStreamAttributes()
	}
	for _, kv := range extras {
		applyExtraAttribute(s.span, kv)
	}
	if captured {
		SetOutputMessages(s.span, content, reasoning, collected)
	}
	s.span.End()
}

// hasChunkPayload reports whether resp carries any output payload (text,
// reasoning, tool call); empty keep-alives don't advance per-chunk metrics.
func hasChunkPayload(resp *chat.MessageStreamResponse) bool {
	for i := range resp.Choices {
		d := &resp.Choices[i].Delta
		if d.Content != "" || d.ReasoningContent != "" || d.ThinkingSignature != "" {
			return true
		}
		if len(d.ToolCalls) > 0 || d.FunctionCall != nil {
			return true
		}
	}
	return false
}
