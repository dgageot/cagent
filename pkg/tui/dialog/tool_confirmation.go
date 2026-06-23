package dialog

import (
	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"
	"github.com/charmbracelet/x/ansi"

	"github.com/docker/docker-agent/pkg/runtime"
	"github.com/docker/docker-agent/pkg/tools"
	"github.com/docker/docker-agent/pkg/tui/components/messages"
	"github.com/docker/docker-agent/pkg/tui/components/toolconfirm"
	"github.com/docker/docker-agent/pkg/tui/core"
	"github.com/docker/docker-agent/pkg/tui/core/layout"
	tuimessages "github.com/docker/docker-agent/pkg/tui/messages"
	"github.com/docker/docker-agent/pkg/tui/service"
	"github.com/docker/docker-agent/pkg/tui/styles"
	"github.com/docker/docker-agent/pkg/tui/types"
)

// Layout constants for tool confirmation dialog.
const (
	toolConfirmDialogWidthPercent  = 70 // Dialog width as percentage of screen
	toolConfirmDialogHeightPercent = 80 // Max dialog height as percentage of screen
	toolConfirmMinScrollHeight     = 5  // Minimum height for the scroll view
	toolConfirmEmptyLinesBefore    = 2  // Empty lines before question
	toolConfirmEmptyLinesAfter     = 1  // Empty lines after question
)

type (
	RuntimeResumeMsg struct {
		Request runtime.ResumeRequest
	}
)

// ToolConfirmationResponse represents the user's response to tool confirmation
type ToolConfirmationResponse struct {
	Response string // "approve", "reject", or "approve-session"
}

type toolConfirmationDialog struct {
	BaseDialog

	msg               *runtime.ToolCallConfirmationEvent
	keyMap            toolconfirm.KeyMap
	sessionState      *service.SessionState
	scrollView        messages.Model
	permissionPattern string // cached permission pattern for this tool call
}

// dialogDimensions returns computed dialog width and content width.
func (d *toolConfirmationDialog) dialogDimensions() (dialogWidth, contentWidth int) {
	dialogWidth = d.Width() * toolConfirmDialogWidthPercent / 100
	contentWidth = dialogWidth - styles.DialogStyle.GetHorizontalFrameSize()
	return dialogWidth, contentWidth
}

func renderBlastRadiusLevel(level tools.BlastRadiusLevel) string {
	return blastRadiusLevelStyle(level).Render(string(level))
}

func blastRadiusLevelStyle(level tools.BlastRadiusLevel) lipgloss.Style {
	switch level {
	case tools.BlastRadiusLow:
		return lipgloss.NewStyle().Foreground(styles.Success).Bold(true)
	case tools.BlastRadiusMedium:
		return lipgloss.NewStyle().Foreground(styles.Warning).Bold(true)
	case tools.BlastRadiusHigh:
		return lipgloss.NewStyle().Foreground(styles.Error).Bold(true)
	default:
		return lipgloss.NewStyle().Foreground(styles.TextSecondary).Bold(true)
	}
}

func renderConfirmationTitle(safety *tools.ToolCallSafety, contentWidth int) string {
	style := styles.DialogTitleStyle.Width(contentWidth)
	if safety != nil && safety.Destructive {
		style = style.Foreground(styles.Warning)
	}
	return style.Render(toolconfirm.Title(safety))
}

func renderConfirmationQuestion(safety *tools.ToolCallSafety, contentWidth int) string {
	if safety == nil || !safety.Destructive {
		return styles.DialogQuestionStyle.Width(contentWidth).Render(toolconfirm.Question(safety))
	}
	level := renderBlastRadiusLevel(toolconfirm.BlastRadiusLevel(safety))
	question := "This is a destructive tool call with blast radius level: " + level + ". Do you want to allow it?"
	return styles.DialogQuestionStyle.Width(contentWidth).Render(question)
}

// SetSize implements [Dialog].
func (d *toolConfirmationDialog) SetSize(width, height int) tea.Cmd {
	d.BaseDialog.SetSize(width, height)

	// Calculate dialog dimensions using helper
	_, contentWidth := d.dialogDimensions()
	maxDialogHeight := height * toolConfirmDialogHeightPercent / 100

	// Measure fixed UI elements using the same rendering as View()
	title := renderConfirmationTitle(d.msg.Safety, contentWidth)
	titleHeight := lipgloss.Height(title)

	separator := d.renderSeparator(contentWidth)
	separatorHeight := lipgloss.Height(separator)

	question := renderConfirmationQuestion(d.msg.Safety, contentWidth)
	questionHeight := lipgloss.Height(question)

	options := d.renderOptions(contentWidth)
	optionsHeight := lipgloss.Height(options)

	// Calculate available height for scroll view
	frameHeight := styles.DialogStyle.GetVerticalFrameSize()
	fixedContentHeight := titleHeight + separatorHeight + toolConfirmEmptyLinesBefore + questionHeight + toolConfirmEmptyLinesAfter + optionsHeight
	availableHeight := max(maxDialogHeight-frameHeight-fixedContentHeight, toolConfirmMinScrollHeight)
	d.scrollView.SetSize(contentWidth, availableHeight)

	return nil
}

// renderSeparator renders the separator line consistently.
func (d *toolConfirmationDialog) renderSeparator(contentWidth int) string {
	return RenderSeparator(contentWidth)
}

// renderOptions renders the Y/N/T/A decision row.
func (d *toolConfirmationDialog) renderOptions(contentWidth int) string {
	return RenderHelpKeys(contentWidth, toolconfirm.OptionsHelp(d.permissionPattern)...)
}

// NewToolConfirmationDialog creates a new tool confirmation dialog
func NewToolConfirmationDialog(msg *runtime.ToolCallConfirmationEvent, sessionState *service.SessionState) Dialog {
	// Create scrollable view with minimal initial size (will be updated in SetSize)
	scrollView := messages.NewScrollableView(1, 1, sessionState)

	// Add the tool call message to the view
	scrollView.AddOrUpdateToolCall(
		"", // agentName - empty for dialog context
		msg.ToolCall,
		msg.ToolDefinition,
		types.ToolStatusConfirmation,
	)

	// Build and cache the permission pattern for display and use
	pattern := toolconfirm.BuildPermissionPattern(msg.ToolCall)

	return &toolConfirmationDialog{
		msg:               msg,
		sessionState:      sessionState,
		keyMap:            toolconfirm.DefaultKeyMap(),
		scrollView:        scrollView,
		permissionPattern: pattern,
	}
}

// Init initializes the tool confirmation dialog
func (d *toolConfirmationDialog) Init() tea.Cmd {
	return d.scrollView.Init()
}

// executeAction dispatches a confirmation decision.
func (d *toolConfirmationDialog) executeAction(decision toolconfirm.Decision) (layout.Model, tea.Cmd) {
	switch decision {
	case toolconfirm.Approve:
		return d, tea.Sequence(
			core.CmdHandler(CloseDialogMsg{}),
			core.CmdHandler(RuntimeResumeMsg{Request: toolconfirm.Approve.Resume("", "")}),
		)
	case toolconfirm.Reject:
		return d, core.CmdHandler(OpenDialogMsg{
			Model: NewToolRejectionReasonDialog(),
		})
	case toolconfirm.ApproveTool:
		return d, tea.Sequence(
			core.CmdHandler(CloseDialogMsg{}),
			core.CmdHandler(RuntimeResumeMsg{Request: toolconfirm.ApproveTool.Resume(d.permissionPattern, "")}),
		)
	case toolconfirm.ApproveSession:
		d.sessionState.SetYoloMode(true)
		return d, tea.Sequence(
			core.CmdHandler(CloseDialogMsg{}),
			core.CmdHandler(RuntimeResumeMsg{Request: toolconfirm.ApproveSession.Resume("", "")}),
		)
	}
	return d, nil
}

// Update handles messages for the tool confirmation dialog
func (d *toolConfirmationDialog) Update(msg tea.Msg) (layout.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		cmd := d.SetSize(msg.Width, msg.Height)
		return d, cmd

	case tea.MouseClickMsg:
		if msg.Button == tea.MouseLeft {
			return d.handleMouseClick(msg)
		}
		return d, nil

	case tea.KeyPressMsg:
		if cmd := HandleQuit(msg); cmd != nil {
			return d, cmd
		}

		if decision, ok := d.keyMap.DecisionFor(msg); ok {
			return d.executeAction(decision)
		}

		// Forward scrolling keys to the scroll view
		if _, isScrollKey := core.GetScrollDirection(msg); isScrollKey {
			updatedScrollView, cmd := d.scrollView.Update(msg)
			d.scrollView = updatedScrollView.(messages.Model)
			return d, cmd
		}

	case tuimessages.WheelCoalescedMsg:
		updatedScrollView, cmd := d.scrollView.Update(msg)
		d.scrollView = updatedScrollView.(messages.Model)
		return d, cmd
	}

	return d, nil
}

// handleMouseClick handles mouse clicks on the action buttons (Y/N/T/A).
func (d *toolConfirmationDialog) handleMouseClick(msg tea.MouseClickMsg) (layout.Model, tea.Cmd) {
	dialogRow, dialogCol := d.Position()
	renderedDialog := d.View()
	dialogHeight := lipgloss.Height(renderedDialog)

	// The options line is the last content line inside the dialog.
	if msg.Y != ContentEndRow(dialogRow, dialogHeight) {
		return d, nil
	}

	// Render the help keys and strip ANSI to get plain text for hit-testing.
	_, contentWidth := d.dialogDimensions()
	options := d.renderOptions(contentWidth)
	optionsPlain := ansi.Strip(options)

	// Content starts after left border + padding.
	frameLeft := styles.DialogStyle.GetBorderLeftSize() + styles.DialogStyle.GetPaddingLeft()

	// The help text is center-aligned within contentWidth.
	plainLen := len(optionsPlain)
	leadingSpaces := max(0, (contentWidth-plainLen)/2)
	relX := msg.X - dialogCol - frameLeft - leadingSpaces
	if relX < 0 || relX >= plainLen {
		return d, nil
	}

	// Walk backward from the click position to find the nearest action key.
	// The plain text looks like: "Y yes  N no  T always allow...  A all tools"
	// Each region starts with its uppercase action key.
	for i := relX; i >= 0; i-- {
		if decision, ok := toolconfirm.DecisionForAction(string(optionsPlain[i])); ok {
			return d.executeAction(decision)
		}
	}

	return d, nil
}

// View renders the tool confirmation dialog
func (d *toolConfirmationDialog) View() string {
	dialogWidth, contentWidth := d.dialogDimensions()

	dialogStyle := styles.DialogStyle.Width(dialogWidth)

	title := renderConfirmationTitle(d.msg.Safety, contentWidth)

	// Separator
	separator := d.renderSeparator(contentWidth)

	// Get scrollable tool call view
	argumentsSection := d.scrollView.View()

	// Combine all parts with proper spacing
	parts := []string{title, separator}

	if argumentsSection != "" {
		parts = append(parts, "", argumentsSection)
	}

	// Confirmation prompt
	question := renderConfirmationQuestion(d.msg.Safety, contentWidth)
	options := d.renderOptions(contentWidth)

	parts = append(parts, "", question, "", options)

	content := lipgloss.JoinVertical(lipgloss.Left, parts...)

	return dialogStyle.Render(content)
}

// Position calculates the position to center the dialog
func (d *toolConfirmationDialog) Position() (row, col int) {
	dialogWidth, _ := d.dialogDimensions()
	renderedDialog := d.View()
	dialogHeight := lipgloss.Height(renderedDialog)
	return CenterPosition(d.Width(), d.Height(), dialogWidth, dialogHeight)
}
