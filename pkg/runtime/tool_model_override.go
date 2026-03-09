package runtime

import (
	"log/slog"

	"github.com/docker/docker-agent/pkg/tools"
)

// resolveToolCallModelOverride returns the per-toolset model override from the
// given tool calls, or "" if none. When multiple tools specify different
// overrides, the first one wins.
func resolveToolCallModelOverride(calls []tools.ToolCall, agentTools []tools.Tool) string {
	if len(calls) == 0 {
		return ""
	}

	toolMap := make(map[string]tools.Tool, len(agentTools))
	for _, t := range agentTools {
		toolMap[t.Name] = t
	}

	for _, call := range calls {
		if t, ok := toolMap[call.Function.Name]; ok && t.ModelOverride != "" {
			slog.Debug("Per-tool model override detected",
				"tool", call.Function.Name, "model", t.ModelOverride)
			return t.ModelOverride
		}
	}

	return ""
}
