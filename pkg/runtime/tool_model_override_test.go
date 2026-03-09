package runtime

import (
	"testing"

	"github.com/stretchr/testify/assert"

	"github.com/docker/docker-agent/pkg/tools"
)

func TestResolveToolCallModelOverride_NoCalls(t *testing.T) {
	result := resolveToolCallModelOverride(nil, nil)
	assert.Empty(t, result)
}

func TestResolveToolCallModelOverride_NoOverride(t *testing.T) {
	agentTools := []tools.Tool{
		{Name: "read_file"},
		{Name: "write_file"},
	}
	calls := []tools.ToolCall{
		{Function: tools.FunctionCall{Name: "read_file"}},
	}

	result := resolveToolCallModelOverride(calls, agentTools)
	assert.Empty(t, result)
}

func TestResolveToolCallModelOverride_SingleOverride(t *testing.T) {
	agentTools := []tools.Tool{
		{Name: "read_file", ModelOverride: "openai/gpt-4o-mini"},
		{Name: "write_file"},
	}
	calls := []tools.ToolCall{
		{Function: tools.FunctionCall{Name: "read_file"}},
	}

	result := resolveToolCallModelOverride(calls, agentTools)
	assert.Equal(t, "openai/gpt-4o-mini", result)
}

func TestResolveToolCallModelOverride_FirstOverrideWins(t *testing.T) {
	agentTools := []tools.Tool{
		{Name: "read_file", ModelOverride: "openai/gpt-4o-mini"},
		{Name: "search_kb", ModelOverride: "anthropic/claude-haiku"},
	}
	calls := []tools.ToolCall{
		{Function: tools.FunctionCall{Name: "read_file"}},
		{Function: tools.FunctionCall{Name: "search_kb"}},
	}

	result := resolveToolCallModelOverride(calls, agentTools)
	assert.Equal(t, "openai/gpt-4o-mini", result)
}

func TestResolveToolCallModelOverride_MixedOverrideAndNonOverride(t *testing.T) {
	agentTools := []tools.Tool{
		{Name: "read_file"},
		{Name: "search_kb", ModelOverride: "openai/gpt-4o-mini"},
	}
	calls := []tools.ToolCall{
		{Function: tools.FunctionCall{Name: "read_file"}},
		{Function: tools.FunctionCall{Name: "search_kb"}},
	}

	// read_file has no override, search_kb does. Since read_file is first
	// but has no override, we skip it and use search_kb's.
	result := resolveToolCallModelOverride(calls, agentTools)
	assert.Equal(t, "openai/gpt-4o-mini", result)
}

func TestResolveToolCallModelOverride_UnknownTool(t *testing.T) {
	agentTools := []tools.Tool{
		{Name: "read_file"},
	}
	calls := []tools.ToolCall{
		{Function: tools.FunctionCall{Name: "unknown_tool"}},
	}

	result := resolveToolCallModelOverride(calls, agentTools)
	assert.Empty(t, result)
}
