package runtime

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"strings"

	"github.com/docker/cagent/pkg/agent"
	"github.com/docker/cagent/pkg/chat"
	"github.com/docker/cagent/pkg/session"
	"github.com/docker/cagent/pkg/team"
	"github.com/docker/cagent/pkg/tools"
)

type ResumeType string

const (
	ResumeTypeApprove        ResumeType = "approve"
	ResumeTypeApproveSession ResumeType = "approve-session"
	ResumeTypeReject         ResumeType = "reject"
)

// ToolHandler is a function type for handling tool calls
type ToolHandler func(ctx context.Context, sess *session.Session, toolCall tools.ToolCall, events chan Event) (*tools.ToolCallResult, error)

// Runtime manages the execution of agents
type Runtime struct {
	logger       *slog.Logger
	toolMap      map[string]ToolHandler
	team         *team.Team
	currentAgent string
	resumeChan   chan ResumeType
	autoRunTools bool
}

type Opt func(*Runtime)

func WithCurrentAgent(agentName string) Opt {
	return func(r *Runtime) {
		r.currentAgent = agentName
	}
}

func WithAutoRunTools(autoRunTools bool) Opt {
	return func(r *Runtime) {
		r.autoRunTools = autoRunTools
	}
}

// New creates a new runtime for an agent and its team
func New(logger *slog.Logger, agents *team.Team, opts ...Opt) *Runtime {
	r := &Runtime{
		toolMap:      make(map[string]ToolHandler),
		team:         agents,
		logger:       logger,
		currentAgent: "root",
		resumeChan:   make(chan ResumeType),
	}

	for _, opt := range opts {
		opt(r)
	}

	logger.Debug("Creating new runtime", "agent", r.currentAgent, "available_agents", agents.Size())

	return r
}

func (r *Runtime) Team() *team.Team {
	return r.team
}

// registerDefaultTools registers the default tool handlers
func (r *Runtime) registerDefaultTools() {
	r.logger.Debug("Registering default tools")
	r.toolMap["transfer_task"] = r.handleTaskTransfer
	r.logger.Debug("Registered default tools", "count", len(r.toolMap))
}

func (r *Runtime) CurrentAgent() *agent.Agent {
	return r.team.Agent(r.currentAgent)
}

// Run starts the agent's interaction loop
func (r *Runtime) RunStream(ctx context.Context, sess *session.Session) <-chan Event {
	r.logger.Debug("Starting runtime stream", "agent", r.currentAgent, "session_id", sess.ID)
	events := make(chan Event)

	go func() {
		defer close(events)
		defer r.logger.Debug("Runtime stream completed", "agent", r.currentAgent, "session_id", sess.ID)

		a := r.team.Agent(r.currentAgent)

		model := a.Model()
		r.logger.Debug("Using agent", "agent", a.Name(), "model", model)
		r.registerDefaultTools()

		for {
			r.logger.Debug("Starting conversation loop iteration", "agent", a.Name())
			messages := sess.GetMessages(a)
			r.logger.Debug("Retrieved messages for processing", "agent", a.Name(), "message_count", len(messages))

			agentTools, err := a.Tools(ctx)
			if err != nil {
				r.logger.Error("Failed to get agent tools", "agent", a.Name(), "error", err)
				events <- Error(fmt.Errorf("failed to get tools: %w", err))
				return
			}
			r.logger.Debug("Retrieved agent tools", "agent", a.Name(), "tool_count", len(agentTools))

			r.logger.Debug("Creating chat completion stream", "agent", a.Name())
			stream, err := model.CreateChatCompletionStream(ctx, messages, agentTools)
			if err != nil {
				r.logger.Error("Failed to create chat completion stream", "agent", a.Name(), "error", err)
				events <- Error(fmt.Errorf("creating chat completion: %w", err))
				return
			}

			r.logger.Debug("Processing stream", "agent", a.Name())
			calls, content, stopped, err := r.handleStream(stream, a, events)
			if err != nil {
				r.logger.Error("Error handling stream", "agent", a.Name(), "error", err)
				events <- Error(err)
				return
			}
			r.logger.Debug("Stream processed", "agent", a.Name(), "tool_calls", len(calls), "content_length", len(content), "stopped", stopped)

			// Add assistant message to conversation history
			assistantMessage := chat.Message{
				Role:      chat.MessageRoleAssistant,
				Content:   content,
				ToolCalls: calls,
			}

			sess.Messages = append(sess.Messages, session.NewAgentMessage(a, &assistantMessage))
			r.logger.Debug("Added assistant message to session", "agent", a.Name(), "total_messages", len(sess.Messages))

			if stopped {
				r.logger.Debug("Conversation stopped", "agent", a.Name())
				break
			}

			if err := r.processToolCalls(ctx, sess, calls, events); err != nil {
				events <- Error(err)
				return
			}
		}
	}()

	return events
}

func (r *Runtime) Resume(ctx context.Context, confirmationType string) {
	r.logger.Debug("Resuming runtime", "agent", r.currentAgent, "confirmation_type", confirmationType)

	cType := ResumeTypeApproveSession
	switch confirmationType {
	case "approve":
		cType = ResumeTypeApprove
	case "reject":
		cType = ResumeTypeReject
	}

	select {
	case r.resumeChan <- cType:
		r.logger.Debug("Resume signal sent", "agent", r.currentAgent)
	default:
		r.logger.Debug("Resume channel not ready, ignoring", "agent", r.currentAgent)
	}
}

// Run starts the agent's interaction loop
func (r *Runtime) Run(ctx context.Context, sess *session.Session) ([]session.Message, error) {
	eventsChan := r.RunStream(ctx, sess)

	for event := range eventsChan {
		if errEvent, ok := event.(*ErrorEvent); ok {
			return nil, errEvent.Error
		}
	}

	return sess.Messages, nil
}

// handleStream handles the stream processing
func (r *Runtime) handleStream(stream chat.MessageStream, a *agent.Agent, events chan Event) (calls []tools.ToolCall, content string, stopped bool, err error) {
	defer stream.Close()

	var fullContent strings.Builder
	var toolCalls []tools.ToolCall

	for {
		response, err := stream.Recv()
		if errors.Is(err, io.EOF) {
			break
		}
		if err != nil {
			return nil, "", true, fmt.Errorf("error receiving from stream: %w", err)
		}

		choice := response.Choices[0]
		if choice.FinishReason == chat.FinishReasonStop {
			return toolCalls, fullContent.String(), true, nil
		}

		// Handle tool calls
		if len(choice.Delta.ToolCalls) > 0 {
			for len(toolCalls) < len(choice.Delta.ToolCalls) {
				toolCalls = append(toolCalls, tools.ToolCall{})
			}

			// Update tool calls with the delta
			for _, deltaToolCall := range choice.Delta.ToolCalls {
				if deltaToolCall.Index == nil {
					continue
				}

				idx := *deltaToolCall.Index
				if idx >= len(toolCalls) {
					newToolCalls := make([]tools.ToolCall, idx+1)
					copy(newToolCalls, toolCalls)
					toolCalls = newToolCalls
				}

				// Update fields based on what's in the delta
				if deltaToolCall.ID != "" {
					toolCalls[idx].ID = deltaToolCall.ID
				}
				if deltaToolCall.Type != "" {
					toolCalls[idx].Type = deltaToolCall.Type
				}
				if deltaToolCall.Function.Name != "" {
					toolCalls[idx].Function.Name = deltaToolCall.Function.Name
				}
				if deltaToolCall.Function.Arguments != "" {
					if toolCalls[idx].Function.Arguments == "" {
						toolCalls[idx].Function.Arguments = deltaToolCall.Function.Arguments
					} else {
						toolCalls[idx].Function.Arguments += deltaToolCall.Function.Arguments
					}
				}
			}
			continue
		}

		if choice.Delta.Content != "" {
			events <- AgentChoice(a.Name(), choice)
			fullContent.WriteString(choice.Delta.Content)
		}
	}

	return toolCalls, fullContent.String(), false, nil
}

// processToolCalls handles the execution of tool calls for an agent
func (r *Runtime) processToolCalls(ctx context.Context, sess *session.Session, calls []tools.ToolCall, events chan Event) error {
	if len(calls) == 0 {
		return nil
	}

	a := r.CurrentAgent()
	r.logger.Debug("Processing tool calls", "agent", a.Name(), "call_count", len(calls))
	agentTools, err := a.Tools(ctx)
	if err != nil {
		r.logger.Error("Failed to get tools for tool calls", "agent", a.Name(), "error", err)
		return fmt.Errorf("failed to get tools: %w", err)
	}

	for _, toolCall := range calls {
		r.logger.Debug("Processing tool call", "agent", a.Name(), "tool", toolCall.Function.Name, "session_id", sess.ID)
		handler, exists := r.toolMap[toolCall.Function.Name]
		if exists {
			r.logger.Debug("Using runtime tool handler", "tool", toolCall.Function.Name, "session_id", sess.ID)
			if sess.ToolsApproved || r.autoRunTools || toolCall.Function.Name == "transfer_task" {
				r.runAgentTool(ctx, handler, sess, toolCall, events, a)
			} else {
				r.logger.Debug("Tools not approved, waiting for resume", "tool", toolCall.Function.Name, "session_id", sess.ID)
				events <- ToolCallConfirmation(toolCall)

				select {
				case cType := <-r.resumeChan:
					switch cType {
					case ResumeTypeApprove:
						r.logger.Debug("Resume signal received, approving tool handler", "tool", toolCall.Function.Name, "session_id", sess.ID)
						r.runAgentTool(ctx, handler, sess, toolCall, events, a)
					case ResumeTypeApproveSession:
						r.logger.Debug("Resume signal received, approving session", "tool", toolCall.Function.Name, "session_id", sess.ID)
						sess.ToolsApproved = true
						r.runAgentTool(ctx, handler, sess, toolCall, events, a)
					case ResumeTypeReject:
						r.logger.Debug("Resume signal received, rejecting tool handler", "tool", toolCall.Function.Name, "session_id", sess.ID)
						r.addToolRejectedResponse(sess, toolCall, events)
					}
				case <-ctx.Done():
					r.logger.Debug("Context cancelled while waiting for resume", "tool", toolCall.Function.Name, "session_id", sess.ID)
					return fmt.Errorf("context cancelled while waiting for resume: %w", ctx.Err())
				}
			}
		}

	toolLoop:
		for _, tool := range agentTools {
			if _, ok := r.toolMap[tool.Function.Name]; ok {
				continue
			}
			if tool.Function.Name != toolCall.Function.Name {
				continue
			}
			r.logger.Debug("Using agent tool handler", "tool", toolCall.Function.Name)

			if sess.ToolsApproved || r.autoRunTools || (tool.Function.Annotations.ReadOnlyHint != nil && *tool.Function.Annotations.ReadOnlyHint == true) {
				r.logger.Debug("Tools approved, running tool", "tool", toolCall.Function.Name, "session_id", sess.ID)
				r.runTool(ctx, tool, toolCall, events, sess, a)
			} else {
				r.logger.Debug("Tools not approved, waiting for resume", "tool", toolCall.Function.Name, "session_id", sess.ID)
				events <- ToolCallConfirmation(toolCall)
				select {
				case cType := <-r.resumeChan:
					switch cType {
					case ResumeTypeApprove:
						r.logger.Debug("Resume signal received, approving tool handler", "tool", toolCall.Function.Name, "session_id", sess.ID)
						r.runTool(ctx, tool, toolCall, events, sess, a)
					case ResumeTypeApproveSession:
						r.logger.Debug("Resume signal received, approving session", "tool", toolCall.Function.Name, "session_id", sess.ID)
						sess.ToolsApproved = true
						r.runTool(ctx, tool, toolCall, events, sess, a)
					case ResumeTypeReject:
						r.logger.Debug("Resume signal received, rejecting tool handler", "tool", toolCall.Function.Name, "session_id", sess.ID)
						r.addToolRejectedResponse(sess, toolCall, events)
					}

					r.logger.Debug("Added tool response to session", "tool", toolCall.Function.Name, "session_id", sess.ID, "total_messages", len(sess.Messages))
					break toolLoop
				case <-ctx.Done():
					r.logger.Debug("Context cancelled while waiting for resume", "tool", toolCall.Function.Name, "session_id", sess.ID)
					return fmt.Errorf("context cancelled while waiting for resume: %w", ctx.Err())
				}
			}
		}
	}

	return nil
}

func (r *Runtime) runTool(ctx context.Context, tool tools.Tool, toolCall tools.ToolCall, events chan Event, sess *session.Session, a *agent.Agent) {
	events <- ToolCall(toolCall)
	res, err := tool.Handler(ctx, toolCall)
	if err != nil {
		r.logger.Error("Error calling tool", "tool", toolCall.Function.Name, "error", err)
		res = &tools.ToolCallResult{
			Output: fmt.Sprintf("Error calling tool: %v", err),
		}
	} else {
		r.logger.Debug("Agent tool call completed", "tool", toolCall.Function.Name, "output_length", len(res.Output))
	}

	events <- ToolCallResponse(toolCall, res.Output)
	toolResponseMsg := chat.Message{
		Role:       chat.MessageRoleTool,
		Content:    res.Output,
		ToolCallID: toolCall.ID,
	}
	sess.Messages = append(sess.Messages, session.NewAgentMessage(a, &toolResponseMsg))
}

func (r *Runtime) runAgentTool(ctx context.Context, handler ToolHandler, sess *session.Session, toolCall tools.ToolCall, events chan Event, a *agent.Agent) {
	events <- ToolCall(toolCall)
	res, err := handler(ctx, sess, toolCall, events)
	if err != nil {
		r.logger.Error("Error executing tool", "tool", toolCall.Function.Name, "error", err)
	} else {
		r.logger.Debug("Tool executed successfully", "tool", toolCall.Function.Name)
	}

	events <- ToolCallResponse(toolCall, res.Output)
	toolResponseMsg := chat.Message{
		Role:       chat.MessageRoleTool,
		Content:    res.Output,
		ToolCallID: toolCall.ID,
	}
	sess.Messages = append(sess.Messages, session.NewAgentMessage(a, &toolResponseMsg))
}

func (r *Runtime) addToolRejectedResponse(sess *session.Session, toolCall tools.ToolCall, events chan Event) {
	a := r.CurrentAgent()

	result := "The user rejected the tool call."

	events <- ToolCallResponse(toolCall, result)

	toolResponseMsg := chat.Message{
		Role:       chat.MessageRoleTool,
		Content:    result,
		ToolCallID: toolCall.ID,
	}
	sess.Messages = append(sess.Messages, session.NewAgentMessage(a, &toolResponseMsg))
}

func (r *Runtime) handleTaskTransfer(ctx context.Context, sess *session.Session, toolCall tools.ToolCall, evts chan Event) (*tools.ToolCallResult, error) {
	var params struct {
		Agent          string `json:"agent"`
		Task           string `json:"task"`
		ExpectedOutput string `json:"expected_output"`
	}

	if err := json.Unmarshal([]byte(toolCall.Function.Arguments), &params); err != nil {
		return nil, fmt.Errorf("invalid arguments: %w", err)
	}

	a := r.CurrentAgent()

	r.logger.Debug("Transferring task to agent", "from_agent", a.Name(), "to_agent", params.Agent, "task", params.Task)

	ca := r.currentAgent
	r.currentAgent = params.Agent

	memberAgentTask := "You are a member of a team of agents. Your goal is to complete the following task:"
	memberAgentTask += fmt.Sprintf("\n\n<task>\n%s\n</task>", params.Task)
	if params.ExpectedOutput != "" {
		memberAgentTask += fmt.Sprintf("\n\n<expected_output>\n%s\n</expected_output>", params.ExpectedOutput)
	}

	s := session.New(sess.GetWorkingDir(), r.logger, session.WithUserMessage(sess.GetMostRecentAgentFilename(), memberAgentTask))
	s.ToolsApproved = sess.ToolsApproved

	for event := range r.RunStream(ctx, s) {
		evts <- event
		if errEvent, ok := event.(*ErrorEvent); ok {
			return nil, errEvent.Error
		}
	}

	r.currentAgent = ca

	r.logger.Debug("Task transfer completed", "agent", params.Agent, "task", params.Task)

	return &tools.ToolCallResult{
		Output: s.Messages[len(s.Messages)-1].Message.Content,
	}, nil
}
