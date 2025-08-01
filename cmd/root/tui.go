package root

import (
	"context"
	"fmt"
	"log/slog"
	"os"
	"strings"
	"time"

	"github.com/charmbracelet/bubbles/spinner"
	"github.com/charmbracelet/bubbles/textinput"
	"github.com/charmbracelet/bubbles/viewport"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/glamour"
	"github.com/charmbracelet/lipgloss"
	"github.com/spf13/cobra"

	"github.com/docker/cagent/pkg/chat"
	"github.com/docker/cagent/pkg/history"
	"github.com/docker/cagent/pkg/loader"
	"github.com/docker/cagent/pkg/runtime"
	"github.com/docker/cagent/pkg/session"
)

var (
	// Styles
	highlight = lipgloss.AdaptiveColor{Light: "#874BFD", Dark: "#7D56F4"}

	statusStyle = lipgloss.NewStyle().
			Foreground(lipgloss.Color("#43BF6D"))

	inputPromptStyle = lipgloss.NewStyle().
				Foreground(highlight)

	errorStyle = lipgloss.NewStyle().
			Foreground(lipgloss.Color("#FF0000"))

	// Viewport styles
	chatViewportStyle = lipgloss.NewStyle().
				BorderStyle(lipgloss.RoundedBorder()).
				BorderForeground(highlight)

	toolViewportStyle = lipgloss.NewStyle().
				BorderStyle(lipgloss.RoundedBorder()).
				BorderForeground(lipgloss.Color("#FFA500")).
				PaddingLeft(0).
				MarginLeft(0).
				Align(lipgloss.Left)

	// Layout styles
	appStyle = lipgloss.NewStyle().
			Padding(1, 0, 0, 0)

	headerStyle = lipgloss.NewStyle().
			Bold(true).
			Foreground(highlight)

	footerStyle = lipgloss.NewStyle()

	// Tool call styles
	toolCallStyle = lipgloss.NewStyle().
			Foreground(lipgloss.Color("#FFA500")).
			Bold(true)

	toolCompletedStyle = lipgloss.NewStyle().
				Foreground(lipgloss.Color("#43BF6D"))
)

// ToolCall represents an active or completed tool call
type ToolCall struct {
	ID          string
	Name        string
	Arguments   string
	IsActive    bool
	IsCompleted bool
	Response    string
	StartTime   time.Time
}

// Message types
type (
	responseMsg     struct{ content string }
	errorMsg        error
	showInputMsg    struct{}
	toolCallMsg     struct{ toolCall ToolCall }
	toolCompleteMsg struct {
		id       string
		response string
	}
	workStartMsg struct{}
	workEndMsg   struct{}
)

// model represents the application state
type model struct {
	// TUI components
	chatViewport viewport.Model
	toolViewport viewport.Model
	textInput    textinput.Model
	spinner      spinner.Model
	renderer     *glamour.TermRenderer

	// Content state
	chatContent    string // rendered chat content
	rawChatContent string // raw markdown chat content
	toolContent    string // rendered tool content
	err            error

	// App state
	ready        bool
	showInput    bool
	width        int
	height       int
	chatHeight   int
	toolHeight   int
	userScrolled bool // Track if user has manually scrolled
	isWorking    bool // Track if LLM is actively working

	// Tool call tracking
	activeToolCalls    map[string]ToolCall
	completedToolCalls []ToolCall

	// Business logic
	rt         *runtime.Runtime
	sess       *session.Session
	responseCh chan string
	toolCh     chan any // Channel for tool events
	history    *history.History
}

// newModel creates and initializes a new model
func newModel(rt *runtime.Runtime, sess *session.Session) (*model, error) {
	// Initialize text input
	ti := textinput.New()
	ti.Placeholder = "Enter your message..."
	ti.Focus()
	ti.CharLimit = 0
	ti.Prompt = inputPromptStyle.Render("> ")

	hist, err := history.New()
	if err != nil {
		return nil, err
	}

	// Create viewports with smooth scrolling
	chatVp := viewport.New(0, 0)
	chatVp.MouseWheelEnabled = true
	chatVp.MouseWheelDelta = 1 // Reduced from 3 to 1 for smoother scrolling

	toolVp := viewport.New(0, 0)
	toolVp.MouseWheelEnabled = true
	toolVp.MouseWheelDelta = 1

	// Initialize spinner
	s := spinner.New()
	s.Spinner = spinner.Dot
	s.Style = lipgloss.NewStyle().Foreground(lipgloss.Color("#FFA500"))

	return &model{
		chatViewport:       chatVp,
		toolViewport:       toolVp,
		textInput:          ti,
		spinner:            s,
		rt:                 rt,
		sess:               sess,
		responseCh:         make(chan string, 100),
		toolCh:             make(chan any, 100),
		history:            hist,
		activeToolCalls:    make(map[string]ToolCall),
		completedToolCalls: make([]ToolCall, 0),
	}, nil
}

func (m *model) updateDimensions(width, height int) {
	m.width = width
	m.height = height

	// Calculate heights
	headerHeight := 1
	footerHeight := 3
	spacingHeight := 1 // Space between the two viewports
	availableHeight := height - headerHeight - footerHeight - spacingHeight

	// Allocate space: 70% for chat, 30% for tools (minimum 5 lines for tools including borders)
	m.chatHeight = int(float64(availableHeight) * 0.7)
	m.toolHeight = availableHeight - m.chatHeight
	if m.toolHeight < 5 {
		m.toolHeight = 5
		m.chatHeight = availableHeight - 5
	}

	// Update chat viewport
	m.chatViewport.Width = width - 2         // Account for borders
	m.chatViewport.Height = m.chatHeight - 2 // Account for borders
	m.chatViewport.Style = chatViewportStyle.
		Width(width).
		Height(m.chatHeight)

	// Update tool viewport
	m.toolViewport.Width = width - 2
	m.toolViewport.Height = m.toolHeight - 2
	m.toolViewport.Style = toolViewportStyle.
		Width(width).
		Height(m.toolHeight).
		PaddingLeft(2).
		MarginLeft(0)

	// Update text input width
	m.textInput.Width = width - 2

	// Update renderer width
	var err error
	m.renderer, err = glamour.NewTermRenderer(
		glamour.WithAutoStyle(),
		glamour.WithWordWrap(width-4), // Account for borders and padding
	)
	if err != nil {
		m.err = err
	}
}

// renderChatContent renders the raw markdown chat content
func (m *model) renderChatContent() error {
	rendered, err := m.renderer.Render(m.rawChatContent)
	if err != nil {
		return err
	}
	m.chatContent = rendered
	m.chatViewport.SetContent(m.chatContent)
	return nil
}

// renderToolContent renders the tool calls content
func (m *model) renderToolContent() {
	var content strings.Builder

	// Show active tool calls
	if len(m.activeToolCalls) > 0 {
		content.WriteString(toolCallStyle.Render("🔧 Active Tool Calls:") + "\n")
		for _, toolCall := range m.activeToolCalls {
			elapsed := time.Since(toolCall.StartTime).Truncate(time.Second)
			content.WriteString(fmt.Sprintf("%s %s(%s) - %v\n",
				m.spinner.View(),
				toolCall.Name,
				truncateWithEllipsis(toolCall.Arguments, 40),
				elapsed))
		}
		content.WriteString("\n")
	}

	// Show recently completed tool calls (last 3)
	if len(m.completedToolCalls) > 0 {
		content.WriteString(toolCompletedStyle.Render("✅ Recent Completions:") + "\n")
		start := max(len(m.completedToolCalls)-3, 0)
		for i := start; i < len(m.completedToolCalls); i++ {
			toolCall := m.completedToolCalls[i]
			content.WriteString(fmt.Sprintf("✓ %s - %s\n",
				toolCall.Name,
				truncateWithEllipsis(toolCall.Response, 50)))
		}
	}

	if content.Len() == 0 {
		content.WriteString("No active tool calls")
	}

	m.toolContent = content.String()
	m.toolViewport.SetContent(m.toolContent)
}

func (m *model) Init() tea.Cmd {
	return tea.Batch(
		textinput.Blink,
		m.spinner.Tick,
	)
}

// Helper function to truncate string with ellipsis
func truncateWithEllipsis(s string, maxLen int) string {
	if len(s) <= maxLen {
		return s
	}
	return s[:maxLen] + "..."
}

func processStream(rt *runtime.Runtime, sess *session.Session, ch chan<- string, toolCh chan<- any) tea.Cmd {
	return func() tea.Msg {
		ctx := context.Background()
		first := true

		// Signal that work has started
		toolCh <- workStartMsg{}

		for event := range rt.RunStream(ctx, sess) {
			switch e := event.(type) {
			case *runtime.AgentChoiceEvent:
				if first {
					ch <- fmt.Sprintf("\n**%s**: ", rt.CurrentAgent().Name())
					first = false
				}
				ch <- e.Choice.Delta.Content
			case *runtime.ToolCallEvent:
				// Send tool start event
				toolCall := ToolCall{
					ID:        e.ToolCall.ID,
					Name:      e.ToolCall.Function.Name,
					Arguments: e.ToolCall.Function.Arguments,
					IsActive:  true,
					StartTime: time.Now(),
				}
				toolCh <- toolCallMsg{toolCall: toolCall}

				// Add to chat content
				ch <- fmt.Sprintf("\n\n> 🔧 **Tool Call**: `%s(%s)`\n\n",
					e.ToolCall.Function.Name,
					truncateWithEllipsis(e.ToolCall.Function.Arguments, 60))

			case *runtime.ToolCallResponseEvent:
				// Send tool completion event
				toolCh <- toolCompleteMsg{id: e.ToolCall.ID, response: e.Response}

				// Add completion to chat content
				ch <- fmt.Sprintf("> ✅ **Completed**: `%s`\n\n",
					truncateWithEllipsis(e.Response, 60))

			case *runtime.ErrorEvent:
				close(ch)
				close(toolCh)
				return errorMsg(e.Error)
			}
		}

		// Signal that work has ended
		toolCh <- workEndMsg{}
		close(ch)
		close(toolCh)
		return nil
	}
}

func readResponse(ch <-chan string) tea.Cmd {
	return func() tea.Msg {
		if msg, ok := <-ch; ok {
			return responseMsg{content: msg}
		}
		return nil
	}
}

func readToolEvents(ch <-chan any) tea.Cmd {
	return func() tea.Msg {
		if msg, ok := <-ch; ok {
			return msg
		}
		return nil
	}
}

func (m *model) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	var cmds []tea.Cmd

	switch msg := msg.(type) {
	case showInputMsg:
		m.showInput = true
		return m, nil

	case tea.WindowSizeMsg:
		if !m.ready {
			m.updateDimensions(msg.Width, msg.Height)
			m.ready = true

			return m, tea.Tick(time.Millisecond*100, func(t time.Time) tea.Msg {
				return showInputMsg{}
			})
		}

		m.updateDimensions(msg.Width, msg.Height)
		if err := m.renderChatContent(); err != nil {
			m.err = err
		}
		m.renderToolContent()

	case tea.KeyMsg:
		if !m.showInput {
			return m, nil
		}

		switch msg.Type {
		case tea.KeyCtrlC:
			return m, tea.Quit
		case tea.KeyUp:
			if msg.Alt {
				// Alt+Up for slow scrolling up
				m.chatViewport.ScrollUp(1)
				m.userScrolled = true
				return m, nil
			}
			m.textInput.SetValue(m.history.Previous())
			return m, nil
		case tea.KeyDown:
			if msg.Alt {
				// Alt+Down for slow scrolling down
				m.chatViewport.ScrollDown(1)
				// Check if we're at the bottom
				if m.chatViewport.AtBottom() {
					m.userScrolled = false
				}
				return m, nil
			}
			m.textInput.SetValue(m.history.Next())
			return m, nil
		case tea.KeyPgUp:
			// Page up for faster scrolling
			m.chatViewport.HalfPageUp()
			m.userScrolled = true
			return m, nil
		case tea.KeyPgDown:
			// Page down for faster scrolling
			m.chatViewport.HalfPageDown()
			// Check if we're at the bottom
			if m.chatViewport.AtBottom() {
				m.userScrolled = false
			}
			return m, nil
		case tea.KeyEnter:
			if strings.TrimSpace(m.textInput.Value()) == "" {
				return m, nil
			}
			cmd := m.handleUserInput()
			return m, cmd
		}

	case responseMsg:
		m.rawChatContent += msg.content
		if err := m.renderChatContent(); err != nil {
			m.err = err
		}
		// Only auto-scroll to bottom if user hasn't manually scrolled up
		if !m.userScrolled {
			m.chatViewport.GotoBottom()
		}
		return m, tea.Tick(time.Millisecond*10, func(t time.Time) tea.Msg {
			return readResponseMsg{}
		})

	case readResponseMsg:
		return m, readResponse(m.responseCh)

	case errorMsg:
		m.err = error(msg)
		return m, nil

	case toolCallMsg:
		m.activeToolCalls[msg.toolCall.ID] = msg.toolCall
		m.renderToolContent()
		return m, readToolEvents(m.toolCh)

	case toolCompleteMsg:
		if toolCall, exists := m.activeToolCalls[msg.id]; exists {
			// Move to completed
			toolCall.IsActive = false
			toolCall.IsCompleted = true
			toolCall.Response = msg.response
			m.completedToolCalls = append(m.completedToolCalls, toolCall)

			// Remove from active
			delete(m.activeToolCalls, msg.id)
			m.renderToolContent()
		}
		return m, readToolEvents(m.toolCh)

	case workStartMsg:
		m.isWorking = true
		return m, readToolEvents(m.toolCh)

	case workEndMsg:
		m.isWorking = false
		return m, readToolEvents(m.toolCh)

	case spinner.TickMsg:
		var spinnerCmd tea.Cmd
		m.spinner, spinnerCmd = m.spinner.Update(msg)
		if len(m.activeToolCalls) > 0 {
			m.renderToolContent()
		}
		return m, spinnerCmd
	}

	// Handle viewport updates and track user scrolling
	var chatVpCmd, toolVpCmd tea.Cmd

	// Store previous position to detect user scrolling
	prevChatY := m.chatViewport.YOffset

	m.chatViewport, chatVpCmd = m.chatViewport.Update(msg)
	m.toolViewport, toolVpCmd = m.toolViewport.Update(msg)

	// Detect if user manually scrolled (position changed via user input, not programmatically)
	if m.chatViewport.YOffset != prevChatY {
		// Check if user scrolled up from bottom
		maxScroll := len(strings.Split(m.chatContent, "\n")) - m.chatViewport.Height
		if maxScroll < 0 {
			maxScroll = 0
		}
		if m.chatViewport.YOffset < maxScroll {
			m.userScrolled = true
		} else {
			m.userScrolled = false
		}
	}

	if chatVpCmd != nil {
		cmds = append(cmds, chatVpCmd)
	}
	if toolVpCmd != nil {
		cmds = append(cmds, toolVpCmd)
	}

	// Handle textinput updates if input is shown
	if m.showInput {
		var tiCmd tea.Cmd
		m.textInput, tiCmd = m.textInput.Update(msg)
		if tiCmd != nil {
			cmds = append(cmds, tiCmd)
		}
	}

	return m, tea.Batch(cmds...)
}

// handleUserInput processes user input and returns appropriate commands
func (m *model) handleUserInput() tea.Cmd {
	input := m.textInput.Value()
	m.textInput.Reset()

	if err := m.history.Add(input); err != nil {
		m.err = err
	}

	userMsg := fmt.Sprintf("\n\n**You**: %s\n", input)
	m.rawChatContent += userMsg
	if err := m.renderChatContent(); err != nil {
		m.err = err
	}
	// Reset scroll state and go to bottom for new user input
	m.userScrolled = false
	m.chatViewport.GotoBottom()

	m.sess.Messages = append(m.sess.Messages, session.NewAgentMessage(m.rt.CurrentAgent(), &chat.Message{
		Role:    chat.MessageRoleUser,
		Content: input,
	}))

	m.responseCh = make(chan string, 100)
	m.toolCh = make(chan any, 100)

	return tea.Batch(
		processStream(m.rt, m.sess, m.responseCh, m.toolCh),
		readResponse(m.responseCh),
		readToolEvents(m.toolCh),
	)
}

// Additional message type for reading responses
type readResponseMsg struct{}

func (m *model) View() string {
	if !m.ready {
		return "Initializing..."
	}

	// Build header
	headerText := "🤖 AI Chat"
	if m.isWorking || len(m.activeToolCalls) > 0 {
		headerText += " " + m.spinner.View() + " Working..."
	}
	header := headerStyle.Render(headerText)

	// Build chat viewport
	chatView := m.chatViewport.View()

	// Build tool viewport
	toolView := m.toolViewport.View()

	// Build footer with status and input
	var footer string
	if m.err != nil {
		footer = errorStyle.Render(fmt.Sprintf("Error: %v", m.err))
	} else {
		status := statusStyle.Render("🤖 Ready\n")
		input := ""
		if m.showInput {
			input = "\n" + m.textInput.View() + "\n"
		}
		footer = footerStyle.Render(status + input)
	}

	// Combine all sections
	return appStyle.Render(
		lipgloss.JoinVertical(
			lipgloss.Left,
			header,
			chatView,
			"", // Empty line for spacing
			toolView,
			footer,
		),
	)
}

// NewTUICmd creates a new TUI command
func NewTUICmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "tui <agent-name>",
		Short: "Run the agent with a TUI",
		Long:  `Run the agent with a Terminal User Interface powered by Charm`,
		Args:  cobra.ExactArgs(1),
		RunE:  runTUICommand,
	}

	cmd.PersistentFlags().StringVarP(&agentName, "agent", "a", "root", "Name of the agent to run")
	cmd.PersistentFlags().BoolVarP(&debugMode, "debug", "d", false, "Enable debug logging")
	cmd.PersistentFlags().StringSliceVar(&runConfig.EnvFiles, "env-from-file", nil, "Set environment variables from file")
	addGatewayFlags(cmd)

	return cmd
}

func runTUICommand(cmd *cobra.Command, args []string) error {
	ctx := context.Background()
	agentFilename := args[0]
	cwd, err := os.Getwd()
	if err != nil {
		return err
	}

	// Configure logger based on debug flag
	logLevel := slog.LevelInfo
	if debugMode {
		logLevel = slog.LevelDebug
	}

	logger := slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{
		Level: logLevel,
	}))

	logger.Debug("Starting agent TUI", "agent", agentName, "debug_mode", debugMode)

	agents, err := loader.Load(ctx, agentFilename, runConfig, logger)
	if err != nil {
		return err
	}
	defer func() {
		if err := agents.StopToolSets(); err != nil {
			logger.Error("Failed to stop tool sets", "error", err)
		}
	}()

	rt := runtime.New(logger, agents, runtime.WithCurrentAgent(agentName))

	m, err := newModel(rt, session.New(cwd, logger))
	if err != nil {
		return err
	}

	p := tea.NewProgram(
		m,
		tea.WithAltScreen(),
		tea.WithMouseAllMotion(), // Enable mouse support
		tea.WithMouseCellMotion(),
	)

	_, err = p.Run()
	return err
}
