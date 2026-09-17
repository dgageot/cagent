package cli

import (
	"bytes"
	"strings"
	"testing"

	"github.com/charmbracelet/x/ansi"
	"gotest.tools/v3/assert"

	"github.com/docker/docker-agent/pkg/chat"
	"github.com/docker/docker-agent/pkg/runtime"
	"github.com/docker/docker-agent/pkg/session"
)

func TestPrintServerToolCall_SanitizesProviderControlSequences(t *testing.T) {
	t.Parallel()

	var buf bytes.Buffer
	NewPrinter(&buf).PrintServerToolCall(chat.ServerToolCall{
		Name:     "code_execution\x1b[31m",
		Language: "python\u0085",
		Input:    "print('a')\n\tprint('b')\x1b]0;title\a",
		Output:   "a\r\nb \u202eevil\u202c",
		IsError:  true,
	})

	got := ansi.Strip(buf.String())
	assert.Assert(t, !strings.ContainsAny(got, "\x1b\a\r\u0085\u202e\u202c"), "raw control sequences leaked: %q", got)
	assert.Assert(t, strings.Contains(got, "Provider ran code_execution_[31m (python_)"), got)
	assert.Assert(t, strings.Contains(got, "print('a')\n\tprint('b')_]0;title_"), "newlines and tabs are kept: %q", got)
	assert.Assert(t, strings.Contains(got, "code_execution_[31m error:\na_\nb _evil_"), got)
}

func TestPrintCitations_SanitizesTitleAndURI(t *testing.T) {
	t.Parallel()

	var buf bytes.Buffer
	NewPrinter(&buf).PrintCitations([]chat.Citation{
		{URI: "https://a.example/x", Title: "Bad\x1b[1mtitle\u2066"},
		{URI: "https://b.example/\u200f"},
	})

	got := ansi.Strip(buf.String())
	assert.Assert(t, strings.Contains(got, "Sources:"), got)
	assert.Assert(t, strings.Contains(got, "1. Bad_[1mtitle_ — https://a.example/x"), got)
	assert.Assert(t, strings.Contains(got, "2. https://b.example/_"), got)
}

// TestRun_HideToolCallsKeepsProviderMetadataOffStdout: --exec writes provider
// metadata to the same stream as the answer, so --hide-tool-calls must drop
// both the cards and the Sources footer for piped, structured output.
func TestRun_HideToolCallsKeepsProviderMetadataOffStdout(t *testing.T) {
	t.Parallel()

	events := []runtime.Event{
		runtime.ServerToolCall("test", "s", chat.ServerToolCall{Name: "google_search", Input: "q"}),
		runtime.AgentCitations("test", "s", []chat.Citation{{URI: "https://a.example", Title: "A"}}),
		runtime.AgentChoice("test", "s", `{"answer":42}`),
	}

	var shown bytes.Buffer
	err := Run(t.Context(), NewPrinter(&shown), Config{}, &mockRuntime{events: events}, session.New(), []string{"hello"})
	assert.NilError(t, err)
	assert.Assert(t, strings.Contains(shown.String(), "Provider ran"), shown.String())
	assert.Assert(t, strings.Contains(shown.String(), "Sources:"), shown.String())

	var hidden bytes.Buffer
	err = Run(t.Context(), NewPrinter(&hidden), Config{HideToolCalls: true}, &mockRuntime{events: events}, session.New(), []string{"hello"})
	assert.NilError(t, err)
	assert.Equal(t, hidden.String(), `{"answer":42}`)
}
