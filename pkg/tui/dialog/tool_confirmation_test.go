package dialog

import (
	"testing"

	"github.com/charmbracelet/x/ansi"
	"github.com/stretchr/testify/assert"

	"github.com/docker/docker-agent/pkg/tools"
	"github.com/docker/docker-agent/pkg/tui/components/toolconfirm"
)

func TestRenderConfirmationTitleWarnsForDestructiveTool(t *testing.T) {
	safety := &tools.ToolCallSafety{Destructive: true, BlastRadius: tools.BlastRadiusHigh}

	rendered := renderConfirmationTitle(safety, 80)

	assert.Contains(t, ansi.Strip(rendered), toolconfirm.DestructiveWarningTitle)
	assert.Contains(t, rendered, "\x1b[")
}

func TestRenderConfirmationQuestionColorsBlastRadius(t *testing.T) {
	safety := &tools.ToolCallSafety{Destructive: true, BlastRadius: tools.BlastRadiusHigh}

	rendered := renderConfirmationQuestion(safety, 80)

	plain := ansi.Strip(rendered)
	assert.Contains(t, plain, "blast radius level: high")
	assert.Contains(t, rendered, "\x1b[")
}
