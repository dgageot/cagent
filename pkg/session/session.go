package session

import (
	"log/slog"
	"strings"
	"time"

	"github.com/google/uuid"

	"github.com/docker/cagent/pkg/agent"
	"github.com/docker/cagent/pkg/chat"
)

// TODO: instead of trimming, we should compact the history when it nears the
// context size of the current LLM
var maxMessages = 100 // Maximum number of messages to keep in context

// Session represents the agent's state including conversation history and variables
type Session struct {
	// ID is the unique identifier for the session
	ID string `json:"id"`

	// Messages holds the conversation history
	Messages []Message `json:"messages"`

	// CreatedAt is the time the session was created
	CreatedAt time.Time `json:"created_at"`

	// ToolsApproved is a flag to indicate if the tools have been approved
	ToolsApproved bool `json:"tools_approved"`

	// WorkingDir is the directory where the session operates
	WorkingDir string `json:"working_dir"`

	// Logger for debugging and logging session operations
	logger *slog.Logger
}

// Message is a message from an agent
type Message struct {
	AgentFilename string       `json:"agentFilename"`
	AgentName     string       `json:"agentName"` // TODO: rename to agent_name
	Message       chat.Message `json:"message"`
}

func UserMessage(agentFilename, content string) Message {
	return Message{
		AgentFilename: agentFilename,
		AgentName:     "",
		Message: chat.Message{
			Role:    chat.MessageRoleUser,
			Content: content,
		},
	}
}

func NewAgentMessage(a *agent.Agent, message *chat.Message) Message {
	return Message{
		AgentFilename: "",
		AgentName:     a.Name(),
		Message:       *message,
	}
}

func SystemMessage(content string) Message {
	return Message{
		AgentFilename: "",
		AgentName:     "",
		Message: chat.Message{
			Role:    chat.MessageRoleSystem,
			Content: content,
		},
	}
}

type Opt func(s *Session)

func WithUserMessage(agentFilename, content string) Opt {
	return func(s *Session) {
		s.Messages = append(s.Messages, UserMessage(agentFilename, content))
	}
}

func WithSystemMessage(content string) Opt {
	return func(s *Session) {
		s.Messages = append(s.Messages, SystemMessage(content))
	}
}

// New creates a new agent session
func New(workingDir string, logger *slog.Logger, opts ...Opt) *Session {
	sessionID := uuid.New().String()
	logger.Debug("Creating new session", "session_id", sessionID)

	s := &Session{
		ID:         sessionID,
		CreatedAt:  time.Now(),
		WorkingDir: workingDir,
		logger:     logger,
	}

	for _, opt := range opts {
		opt(s)
	}

	return s
}

func (s *Session) GetWorkingDir() string {
	return s.WorkingDir
}

func (s *Session) GetMessages(a *agent.Agent) []chat.Message {
	s.logger.Debug("Getting messages for agent", "agent", a.Name(), "session_id", s.ID)

	messages := make([]chat.Message, 0)

	if a.HasSubAgents() || a.HasParents() {
		subAgents := append(a.SubAgents(), a.Parents()...)

		subAgentsStr := ""
		validAgentIDs := make([]string, 0, len(subAgents))
		for _, subAgent := range subAgents {
			subAgentsStr += "ID: " + subAgent.Name() + " | Name: " + subAgent.Name() + " | Description: " + subAgent.Description() + "\n"
			validAgentIDs = append(validAgentIDs, subAgent.Name())
		}

		messages = append(messages, chat.Message{
			Role:    "system",
			Content: "You are a multi-agent system, make sure to answer the user query in the most helpful way possible. You have access to these sub-agents:\n" + subAgentsStr + "\nIMPORTANT: You can ONLY transfer tasks to the agents listed above using their ID. The valid agent IDs are: " + strings.Join(validAgentIDs, ", ") + ". You MUST NOT attempt to transfer to any other agent IDs - doing so will cause system errors.\n\nIf you are the best to answer the question according to your description, you can answer it.\n\nIf another agent is better for answering the question according to its description, call `transfer_task` function to transfer the question to that agent using the agent's ID. When transferring, do not generate any text other than the function call.\n\n",
		})
	}

	content := a.Instruction()

	if a.AddDate() {
		content += "\n\n" + "Today's date: " + time.Now().Format("2006-01-02")
	}

	if a.AddEnvironmentInfo() {
		content += "\n\n" + getEnvironmentInfo(s.GetWorkingDir())
	}

	if promptContent, err := AddPromptFileContent(s.GetWorkingDir(), a.AddPromptFile()); err != nil {
		s.logger.Error("Failed to read prompt file", "error", err)
	} else {
		content += promptContent
	}

	messages = append(messages, chat.Message{
		Role:    chat.MessageRoleSystem,
		Content: content,
	})

	for _, tool := range a.ToolSets() {
		if tool.Instructions() != "" {
			messages = append(messages, chat.Message{
				Role:    chat.MessageRoleSystem,
				Content: tool.Instructions(),
			})
		}
	}

	for i := range s.Messages {
		messages = append(messages, s.Messages[i].Message)
	}

	trimmed := trimMessages(messages)

	s.logger.Debug("Retrieved messages for agent",
		"agent", a.Name(),
		"session_id", s.ID,
		"total_messages", len(messages),
		"trimmed_messages", len(trimmed))

	return trimmed
}

func (s *Session) GetMostRecentAgentFilename() string {
	for i := len(s.Messages) - 1; i >= 0; i-- {
		if agentFilename := s.Messages[i].AgentFilename; agentFilename != "" {
			return agentFilename
		}
	}
	return ""
}

// trimMessages ensures we don't exceed the maximum number of messages while maintaining
// consistency between assistant messages and their tool call results
func trimMessages(messages []chat.Message) []chat.Message {
	if len(messages) <= maxMessages {
		return messages
	}

	// Keep track of tool call IDs that need to be removed
	toolCallsToRemove := make(map[string]bool)

	// Calculate how many messages we need to remove
	toRemove := len(messages) - maxMessages

	// Start from the beginning (oldest messages)
	for i := range toRemove {
		// If this is an assistant message with tool calls, mark them for removal
		if messages[i].Role == chat.MessageRoleAssistant {
			for _, toolCall := range messages[i].ToolCalls {
				toolCallsToRemove[toolCall.ID] = true
			}
		}
	}

	// Filter messages keeping only those we want to keep
	result := make([]chat.Message, 0, maxMessages)
	for i := toRemove; i < len(messages); i++ {
		msg := messages[i]

		// Skip tool messages that correspond to removed assistant messages
		if msg.Role == chat.MessageRoleTool && toolCallsToRemove[msg.ToolCallID] {
			continue
		}

		result = append(result, msg)
	}

	return result
}
