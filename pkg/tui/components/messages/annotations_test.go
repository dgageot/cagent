package messages

import (
	"strings"
	"testing"

	"github.com/charmbracelet/x/ansi"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"

	"github.com/docker/docker-agent/pkg/chat"
	"github.com/docker/docker-agent/pkg/session"
	"github.com/docker/docker-agent/pkg/tui/animation"
	"github.com/docker/docker-agent/pkg/tui/components/reasoningblock"
	"github.com/docker/docker-agent/pkg/tui/service"
	"github.com/docker/docker-agent/pkg/tui/types"
)

func newAnnotationTestModel(t *testing.T) *model {
	t.Helper()
	m := NewScrollableView(animation.NewRuntime(), 80, 40, &service.SessionState{}).(*model)
	m.SetSize(80, 40)
	return m
}

func TestAppendAssistantCitationsRendersSourcesFooterAfterText(t *testing.T) {
	t.Parallel()
	m := newAnnotationTestModel(t)
	m.AppendToLastMessage("root", "Paris is sunny.")
	m.AppendAssistantCitations("root", []chat.Citation{
		{URI: "https://a.example/weather", Title: "Weather A"},
		{URI: "https://a.example/weather", Title: "duplicate"},
		{URI: "https://b.example"},
	})

	require.Len(t, m.messages, 1, "citations join the streamed assistant message")
	assert.Equal(t, []chat.Citation{{URI: "https://a.example/weather", Title: "Weather A"}, {URI: "https://b.example"}}, m.messages[0].Citations)

	plain := ansi.Strip(m.View())
	assert.Contains(t, plain, "Paris is sunny.")
	assert.Contains(t, plain, "Sources:")
	assert.Contains(t, plain, "1. Weather A — https://a.example/weather")
	assert.Contains(t, plain, "2. https://b.example")
	assert.Less(t, strings.Index(plain, "Paris is sunny."), strings.Index(plain, "Sources:"))

	// Streaming more text after the footer arrived keeps a single message.
	m.AppendToLastMessage("root", " Enjoy.")
	require.Len(t, m.messages, 1)
	assert.Contains(t, ansi.Strip(m.View()), "Enjoy.")
}

func TestAppendAssistantCitationsWithoutTextStartsMessage(t *testing.T) {
	t.Parallel()
	m := newAnnotationTestModel(t)
	m.AppendAssistantCitations("root", []chat.Citation{{URI: "https://a.example"}})

	require.Len(t, m.messages, 1)
	assert.Equal(t, types.MessageTypeAssistant, m.messages[0].Type)
	assert.Empty(t, m.messages[0].Content)
	assert.Contains(t, ansi.Strip(m.View()), "Sources:")
}

// TestSearchMetadataBeforeAndAfterTextKeepsSourcesUnderAnswer replays the
// runtime's event order for a grounded Gemini chunk (search card, then
// citations, then the grounded text) with and without earlier assistant
// text: the Sources footer must end up under the answer, not on a
// citation-only message stranded before the card.
func TestSearchMetadataBeforeAndAfterTextKeepsSourcesUnderAnswer(t *testing.T) {
	t.Parallel()

	search := chat.ServerToolCall{Name: "google_search", Input: "weather paris"}
	source := chat.Citation{URI: "https://a.example", Title: "A"}

	t.Run("before text", func(t *testing.T) {
		t.Parallel()
		m := newAnnotationTestModel(t)
		m.AddServerToolCall("root", search)
		m.AppendAssistantCitations("root", []chat.Citation{source})
		m.AppendToLastMessage("root", "Paris is sunny.")

		require.Len(t, m.messages, 2)
		assert.Equal(t, types.MessageTypeToolCall, m.messages[0].Type)
		assert.Equal(t, "Paris is sunny.", m.messages[1].Content)
		assert.Equal(t, []chat.Citation{source}, m.messages[1].Citations)
		plain := ansi.Strip(m.View())
		assert.Less(t, strings.Index(plain, "Paris is sunny."), strings.Index(plain, "Sources:"))
	})

	t.Run("after text", func(t *testing.T) {
		t.Parallel()
		m := newAnnotationTestModel(t)
		m.AppendToLastMessage("root", "Let me check.")
		m.AddServerToolCall("root", search)
		m.AppendAssistantCitations("root", []chat.Citation{source})
		m.AppendToLastMessage("root", "Paris is sunny.")

		require.Len(t, m.messages, 3)
		assert.Equal(t, "Let me check.", m.messages[0].Content)
		assert.Empty(t, m.messages[0].Citations, "text written before the search is not what the sources ground")
		assert.Equal(t, types.MessageTypeToolCall, m.messages[1].Type)
		assert.Equal(t, "Paris is sunny.", m.messages[2].Content)
		assert.Equal(t, []chat.Citation{source}, m.messages[2].Citations)
		plain := ansi.Strip(m.View())
		assert.Less(t, strings.Index(plain, "Paris is sunny."), strings.Index(plain, "Sources:"))
	})
}

func TestAddServerToolCallRendersCompletedCard(t *testing.T) {
	t.Parallel()
	m := newAnnotationTestModel(t)
	m.AddServerToolCall("root", chat.ServerToolCall{Name: "code_execution", Language: "python", Input: "print(6*7)", Output: "42"})
	m.AddServerToolCall("root", chat.ServerToolCall{Name: "code_execution", Language: "python", Input: "1/0", Output: "ZeroDivisionError", IsError: true})
	m.AppendToLastMessage("root", "The answer is 42.")

	require.Len(t, m.messages, 3)
	first, second := m.messages[0], m.messages[1]
	assert.Equal(t, types.MessageTypeToolCall, first.Type)
	assert.Equal(t, types.ToolStatusCompleted, first.ToolStatus)
	assert.Equal(t, "code_execution", first.ToolCall.Function.Name)
	assert.Equal(t, "42", first.Content)
	assert.Nil(t, first.StartedAt, "a completed provider-side call never shows a running timer")
	assert.Equal(t, types.ToolStatusError, second.ToolStatus)
	assert.NotEqual(t, first.ToolCall.ID, second.ToolCall.ID)

	plain := ansi.Strip(m.View())
	assert.Contains(t, plain, "code_execution")
	assert.Contains(t, plain, "print(6*7)")
	assert.Contains(t, plain, "42")
	assert.Contains(t, plain, "ZeroDivisionError")
	assert.Contains(t, plain, "The answer is 42.")
}

func TestAddServerToolCallJoinsActiveReasoningBlock(t *testing.T) {
	t.Parallel()
	m := newAnnotationTestModel(t)
	m.AppendReasoning("root", "Let me compute this.")
	m.AddServerToolCall("root", chat.ServerToolCall{Name: "code_execution", Language: "python", Input: "print(1)", Output: "1"})

	require.Len(t, m.messages, 1, "the card lands inside the reasoning block, like a local tool call")
	block, ok := m.views[0].(*reasoningblock.Model)
	require.True(t, ok)
	assert.Equal(t, 1, block.ToolCount())
}

func TestLoadFromSessionRestoresAnnotations(t *testing.T) {
	t.Parallel()
	m := newAnnotationTestModel(t)
	sess := &session.Session{
		ID: "sess-annotations",
		Messages: []session.Item{
			session.NewMessageItem(&session.Message{Message: chat.Message{Role: chat.MessageRoleUser, Content: "2+2?"}}),
			session.NewMessageItem(&session.Message{
				AgentName: "root",
				Message: chat.Message{
					Role:            chat.MessageRoleAssistant,
					Content:         "It is 4.",
					Citations:       []chat.Citation{{URI: "https://math.example", Title: "Sums"}, {Title: "dropped: no uri"}},
					ServerToolCalls: []chat.ServerToolCall{{Name: "code_execution", Language: "python", Input: "print(2+2)", Output: "4"}},
				},
			}),
		},
	}

	m.LoadFromSession(sess, nil)

	require.Len(t, m.messages, 3)
	assert.Equal(t, types.MessageTypeUser, m.messages[0].Type)
	assert.Equal(t, types.MessageTypeToolCall, m.messages[1].Type, "provider-side call renders before the answer, where it ran")
	assert.Equal(t, "code_execution", m.messages[1].ToolCall.Function.Name)
	assert.Equal(t, "4", m.messages[1].Content)
	assert.Equal(t, types.MessageTypeAssistant, m.messages[2].Type)
	assert.Equal(t, "It is 4.", m.messages[2].Content)
	assert.Equal(t, []chat.Citation{{URI: "https://math.example", Title: "Sums"}}, m.messages[2].Citations)

	plain := ansi.Strip(m.View())
	assert.Contains(t, plain, "print(2+2)")
	assert.Contains(t, plain, "Sources:")
	assert.Contains(t, plain, "Sums — https://math.example")
}

func TestLoadFromSessionCitationsOnlyAssistantMessageIsVisible(t *testing.T) {
	t.Parallel()
	m := newAnnotationTestModel(t)
	sess := &session.Session{
		ID: "sess-citations-only",
		Messages: []session.Item{session.NewMessageItem(&session.Message{
			AgentName: "root",
			Message:   chat.Message{Role: chat.MessageRoleAssistant, Citations: []chat.Citation{{URI: "https://a.example"}}},
		})},
	}

	m.LoadFromSession(sess, nil)

	require.Len(t, m.messages, 1)
	assert.Equal(t, types.MessageTypeAssistant, m.messages[0].Type)
	assert.Contains(t, ansi.Strip(m.View()), "https://a.example")
}
