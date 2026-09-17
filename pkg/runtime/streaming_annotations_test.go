package runtime

import (
	"encoding/json"
	"testing"

	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"

	"github.com/docker/docker-agent/pkg/agent"
	"github.com/docker/docker-agent/pkg/chat"
	"github.com/docker/docker-agent/pkg/session"
)

// AddAnnotations appends a chunk carrying display-only provider annotations
// (grounding citations and/or provider-executed tool calls) alongside
// optional text, the way the Gemini adapter surfaces them.
func (b *streamBuilder) AddAnnotations(content string, citations []chat.Citation, calls ...chat.ServerToolCall) *streamBuilder {
	b.responses = append(b.responses, chat.MessageStreamResponse{
		Choices: []chat.MessageStreamChoice{{
			Delta: chat.MessageDelta{Content: content, Citations: citations, ServerToolCalls: calls},
		}},
	})
	return b
}

// AddAnnotationsWithStop appends a terminal chunk that also carries
// annotations, the way the adapter flushes unfinished code on "done".
func (b *streamBuilder) AddAnnotationsWithStop(citations []chat.Citation, calls ...chat.ServerToolCall) *streamBuilder {
	b.responses = append(b.responses, chat.MessageStreamResponse{
		Choices: []chat.MessageStreamChoice{{
			FinishReason: chat.FinishReasonStop,
			Delta:        chat.MessageDelta{Citations: citations, ServerToolCalls: calls},
		}},
		Usage: &chat.Usage{InputTokens: 1, OutputTokens: 1},
	})
	return b
}

func runAnnotatedStream(t *testing.T, stream *mockStream) (streamResult, []Event) {
	t.Helper()
	a := agent.New("root", "test", agent.WithModel(&mockProvider{id: "test/mock-model", stream: stream}))
	sess := session.New(session.WithUserMessage("go"))
	evCh := make(chan Event, 64)
	res, err := handleStream(
		t.Context(), nil, stream, a, nil, sess, nil,
		defaultTelemetry{}, NewChannelSink(evCh), defaultStreamIdleTimeout,
	)
	require.NoError(t, err)
	return res, collectEvents(evCh)
}

func TestHandleStream_CitationsDeduplicatedAndEmittedOnce(t *testing.T) {
	t.Parallel()

	a := chat.Citation{URI: "https://a.example", Title: "A"}
	b := chat.Citation{URI: "https://b.example", Title: "B"}
	stream := newStreamBuilder().
		AddAnnotations("Paris ", []chat.Citation{a}).
		AddAnnotations("is sunny.", []chat.Citation{a, b, {Title: "no uri"}}).
		AddAnnotationsWithStop([]chat.Citation{a, b}).
		Build()

	res, events := runAnnotatedStream(t, stream)

	assert.Equal(t, "Paris is sunny.", res.Content, "assistant text is untouched by citations")
	assert.Equal(t, []chat.Citation{a, b}, res.Citations)
	assert.Empty(t, res.Calls)
	assert.True(t, res.Stopped)
	assert.Equal(t, chat.FinishReasonStop, res.FinishReason)

	var emitted [][]chat.Citation
	for _, e := range events {
		if ce, ok := e.(*AgentCitationsEvent); ok {
			assert.Equal(t, "root", ce.AgentName)
			emitted = append(emitted, ce.Citations)
		}
	}
	assert.Equal(t, [][]chat.Citation{{a}, {b}}, emitted, "each URI is announced exactly once, in first-seen order")
}

func TestHandleStream_ServerToolCallsSurfacedWithoutBecomingToolCalls(t *testing.T) {
	t.Parallel()

	exec := chat.ServerToolCall{Name: "code_execution", Language: "python", Input: "print(42)", Output: "42\n"}
	search := chat.ServerToolCall{Name: "google_search", Input: "meaning of life"}
	unfinished := chat.ServerToolCall{Name: "code_execution", Language: "python", Input: "while True: pass"}
	stream := newStreamBuilder().
		AddAnnotations("", nil, search).
		AddAnnotations("", nil, exec).
		AddContent("The answer is 42.").
		AddAnnotationsWithStop(nil, unfinished).
		Build()

	res, events := runAnnotatedStream(t, stream)

	assert.Equal(t, "The answer is 42.", res.Content)
	assert.Equal(t, []chat.ServerToolCall{search, exec, unfinished}, res.ServerToolCalls)
	assert.Empty(t, res.Calls, "provider-side execution must never be scheduled as a local tool call")
	assert.True(t, res.Stopped, "a turn with only provider-side tools has nothing left to execute")
	assert.Equal(t, chat.FinishReasonStop, res.FinishReason)
	assert.True(t, res.ResponseStarted)

	var emitted []chat.ServerToolCall
	for _, e := range events {
		switch e := e.(type) {
		case *ServerToolCallEvent:
			assert.Equal(t, "root", e.AgentName)
			emitted = append(emitted, e.ToolCall)
		case *PartialToolCallEvent, *ToolCallEvent, *ToolCallConfirmationEvent:
			t.Fatalf("unexpected local tool event %T", e)
		}
	}
	assert.Equal(t, []chat.ServerToolCall{search, exec, unfinished}, emitted)
}

// TestHandleStream_SearchMetadataOrderedBeforeCitationsAndText pins the
// per-chunk event order for Gemini's grounding shape, where one chunk carries
// the search query, its sources and the grounded text: the card must precede
// the citations so the TUI attaches them to the answer, not to a card.
func TestHandleStream_SearchMetadataOrderedBeforeCitationsAndText(t *testing.T) {
	t.Parallel()

	search := chat.ServerToolCall{Name: "google_search", Input: "weather paris"}
	source := chat.Citation{URI: "https://a.example", Title: "A"}
	stream := newStreamBuilder().
		AddContent("Let me check. ").
		AddAnnotations("Paris is sunny.", []chat.Citation{source}, search).
		AddStopWithUsage(1, 1).
		Build()

	res, events := runAnnotatedStream(t, stream)

	assert.Equal(t, "Let me check. Paris is sunny.", res.Content)
	var order []string
	for _, e := range events {
		switch e := e.(type) {
		case *AgentChoiceEvent:
			order = append(order, "text:"+e.Content)
		case *ServerToolCallEvent:
			order = append(order, "call:"+e.ToolCall.Name)
		case *AgentCitationsEvent:
			order = append(order, "citations")
		}
	}
	assert.Equal(t, []string{"text:Let me check. ", "call:google_search", "citations", "text:Paris is sunny."}, order)
}

func TestHandleStream_ServerToolCallOnlyTurnWithBareEOF(t *testing.T) {
	t.Parallel()

	stream := newStreamBuilder().
		AddAnnotations("", nil, chat.ServerToolCall{Name: "google_search", Input: "q"}).
		Build() // bare EOF, no finish reason

	res, _ := runAnnotatedStream(t, stream)

	assert.Len(t, res.ServerToolCalls, 1)
	assert.True(t, res.Stopped)
	assert.Equal(t, chat.FinishReasonNull, res.FinishReason, "annotations alone are not assistant output")
}

// TestRunStream_AnnotationsPersistedButInertInHistory runs a full turn and
// checks the annotations land on the persisted assistant message (for
// transcripts and session reopen) without touching the content or tool
// calls a provider replays.
func TestRunStream_AnnotationsPersistedButInertInHistory(t *testing.T) {
	t.Parallel()

	citation := chat.Citation{URI: "https://a.example", Title: "A"}
	exec := chat.ServerToolCall{Name: "code_execution", Language: "python", Input: "print(1)", Output: "1\n"}
	sess := session.New(session.WithUserMessage("compute"))
	stream := newStreamBuilder().
		AddAnnotations("", nil, exec).
		AddAnnotations(`{"answer":1}`, []chat.Citation{citation}).
		AddStopWithUsage(1, 1).
		Build()

	events := runSession(t, sess, stream)

	var added *MessageAddedEvent
	for _, e := range events {
		if m, ok := e.(*MessageAddedEvent); ok && m.Message.Message.Role == chat.MessageRoleAssistant {
			added = m
		}
	}
	require.NotNil(t, added)
	assert.Equal(t, `{"answer":1}`, added.Message.Message.Content, "structured content must stay byte-identical")
	assert.Equal(t, []chat.Citation{citation}, added.Message.Message.Citations)
	assert.Equal(t, []chat.ServerToolCall{exec}, added.Message.Message.ServerToolCalls)
	assert.Empty(t, added.Message.Message.ToolCalls)

	for _, msg := range sess.GetAllMessages() {
		assert.NotEqual(t, chat.MessageRoleTool, msg.Message.Role, "no tool result may be synthesized for a provider-side call")
	}
}

// TestAnnotationEvents_RoundTripJSON pins the wire shape consumed by remote
// runtime clients (see NewClient's event registry).
func TestAnnotationEvents_RoundTripJSON(t *testing.T) {
	t.Parallel()

	client, err := NewClient("http://127.0.0.1:0")
	require.NoError(t, err)

	for _, ev := range []Event{
		AgentCitations("root", "s1", []chat.Citation{{URI: "https://a.example", Title: "A"}}),
		ServerToolCall("root", "s1", chat.ServerToolCall{Name: "code_execution", Language: "python", Input: "1", Output: "1", IsError: true}),
	} {
		raw, err := json.Marshal(ev)
		require.NoError(t, err)
		var probe struct {
			Type string `json:"type"`
		}
		require.NoError(t, json.Unmarshal(raw, &probe))
		factory, ok := client.registry[probe.Type]
		require.True(t, ok, "event type %q must be registered for remote decoding", probe.Type)
		decoded := factory()
		require.NoError(t, json.Unmarshal(raw, decoded))
		reencoded, err := json.Marshal(decoded)
		require.NoError(t, err)
		assert.JSONEq(t, string(raw), string(reencoded))
	}
}
