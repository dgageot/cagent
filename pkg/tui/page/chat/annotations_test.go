package chat

import (
	"testing"

	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"

	"github.com/docker/docker-agent/pkg/app"
	"github.com/docker/docker-agent/pkg/chat"
	"github.com/docker/docker-agent/pkg/runtime"
	"github.com/docker/docker-agent/pkg/session"
	"github.com/docker/docker-agent/pkg/tui/animation"
	"github.com/docker/docker-agent/pkg/tui/service"
	"github.com/docker/docker-agent/pkg/tui/types"
)

func newAnnotationTestPage(t *testing.T) *chatPage {
	t.Helper()
	sess := session.New()
	return New(animation.NewRuntime(), t.Context(), app.New(t.Context(), queueTestRuntime{}, sess), service.NewSessionState(sess)).(*chatPage)
}

func TestServerToolCallAndCitationsEventsRenderInTurn(t *testing.T) {
	t.Parallel()
	p := newAnnotationTestPage(t)
	const sessID = "sess-annotations"

	handled, _ := p.handleRuntimeEvent(runtime.StreamStarted(sessID, "root"))
	require.True(t, handled)
	handled, _ = p.handleRuntimeEvent(runtime.ServerToolCall("root", sessID, chat.ServerToolCall{
		Name: "code_execution", Language: "python", Input: "print(42)", Output: "42\n",
	}))
	require.True(t, handled, "ServerToolCallEvent must be a recognized runtime event")
	handled, _ = p.handleRuntimeEvent(runtime.AgentChoice("root", sessID, "The answer is 42."))
	require.True(t, handled)
	handled, _ = p.handleRuntimeEvent(runtime.AgentCitations("root", sessID, []chat.Citation{{URI: "https://a.example", Title: "A"}}))
	require.True(t, handled, "AgentCitationsEvent must be a recognized runtime event")

	assert.Equal(t, 1, p.messages.MessageTypeCount(types.MessageTypeToolCall))
	assert.Equal(t, 1, p.messages.MessageTypeCount(types.MessageTypeAssistant), "citations join the streamed answer")
}

// TestGroundedChunkAfterTextKeepsOneAnswerMessage feeds the runtime's order
// for a grounded chunk that follows earlier text: card, citations, then the
// grounded text. The citations must open the answer message the text then
// joins, instead of leaving a stranded citation-only message.
func TestGroundedChunkAfterTextKeepsOneAnswerMessage(t *testing.T) {
	t.Parallel()
	p := newAnnotationTestPage(t)
	const sessID = "sess-grounded"

	for _, ev := range []runtime.Event{
		runtime.StreamStarted(sessID, "root"),
		runtime.AgentChoice("root", sessID, "Let me check. "),
		runtime.ServerToolCall("root", sessID, chat.ServerToolCall{Name: "google_search", Input: "weather paris"}),
		runtime.AgentCitations("root", sessID, []chat.Citation{{URI: "https://a.example", Title: "A"}}),
		runtime.AgentChoice("root", sessID, "Paris is sunny."),
	} {
		handled, _ := p.handleRuntimeEvent(ev)
		require.True(t, handled)
	}

	assert.Equal(t, 1, p.messages.MessageTypeCount(types.MessageTypeToolCall))
	assert.Equal(t, 2, p.messages.MessageTypeCount(types.MessageTypeAssistant), "text before the card, then one answer carrying the sources")
}

func TestAnnotationEventsIgnoredAfterCancel(t *testing.T) {
	t.Parallel()
	p := newAnnotationTestPage(t)
	p.streamCancelled = true

	handled, cmd := p.handleRuntimeEvent(runtime.ServerToolCall("root", "s", chat.ServerToolCall{Name: "google_search", Input: "q"}))
	require.True(t, handled)
	assert.Nil(t, cmd)
	handled, cmd = p.handleRuntimeEvent(runtime.AgentCitations("root", "s", []chat.Citation{{URI: "https://a.example"}}))
	require.True(t, handled)
	assert.Nil(t, cmd)
	assert.Equal(t, 0, p.messages.MessageTypeCount(types.MessageTypeToolCall))
	assert.Equal(t, 0, p.messages.MessageTypeCount(types.MessageTypeAssistant))
}
