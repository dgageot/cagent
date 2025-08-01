// executor.go handles runtime creation and stream execution for agent sessions
// in cagent's servicecore architecture.
//
// This component bridges the gap between the servicecore's session management
// and cagent's existing runtime system, providing:
//
// 1. Runtime Lifecycle Management:
//   - Creates new runtime instances from resolved agent configurations
//   - Initializes agent toolsets and validates configurations
//   - Manages runtime cleanup to prevent resource leaks
//   - Integrates with existing cagent loader and runtime components
//
// 2. Stream Execution Coordination:
//   - Processes user messages through the agent runtime
//   - Collects streaming events from runtime.RunStream() into structured responses
//   - Tracks execution metadata (duration, tool calls, event counts)
//   - Handles runtime errors and provides structured error reporting
//
// 3. Event Processing and Response Formatting:
//   - Aggregates streaming events (tool calls, responses, agent messages)
//   - Builds structured Response objects with content, events, and metadata
//   - Provides timing and performance metrics for monitoring
//   - Maintains session state consistency across message exchanges
//
// Integration Points:
// - Uses pkg/loader for agent configuration parsing
// - Leverages pkg/runtime for actual agent execution
// - Integrates with pkg/session for conversation state management
// - Provides clean abstractions for both MCP and HTTP transports
//
// The Executor ensures that runtime complexity is hidden from transport layers
// while providing rich structured data for client consumption and system monitoring.
package servicecore

import (
	"context"
	"fmt"
	"log/slog"
	"strings"
	"time"

	latest "github.com/docker/cagent/pkg/config/v1"
	"github.com/docker/cagent/pkg/loader"
	"github.com/docker/cagent/pkg/runtime"
	"github.com/docker/cagent/pkg/session"
)

// Executor handles runtime creation and stream execution
type Executor struct {
	workingDir string
	logger     *slog.Logger
}

// NewExecutor creates a new runtime executor
func NewExecutor(workingDir string, logger *slog.Logger) *Executor {
	return &Executor{
		workingDir: workingDir,
		logger:     logger,
	}
}

// CreateRuntime creates a new runtime instance for an agent
func (e *Executor) CreateRuntime(agentPath, agentName string, envFiles []string, gateway string) (*runtime.Runtime, *session.Session, error) {
	ctx := context.Background()

	e.logger.Debug("Creating runtime", "agent_path", agentPath, "agent_name", agentName)

	// Load agent configuration using existing loader
	runConfig := latest.RuntimeConfig{
		EnvFiles: envFiles,
		Gateway:  gateway,
	}
	agents, err := loader.Load(ctx, agentPath, runConfig, e.logger)
	if err != nil {
		return nil, nil, fmt.Errorf("loading agent configuration: %w", err)
	}

	// Tool sets are started automatically when needed

	// Create runtime
	rt := runtime.New(e.logger, agents, runtime.WithCurrentAgent(agentName))

	// Create session
	sess := session.New(e.workingDir, e.logger)

	e.logger.Debug("Runtime created successfully", "agent_name", agentName, "session_id", sess.ID)
	return rt, sess, nil
}

// ExecuteStream executes a message and collects streaming events into a response
func (e *Executor) ExecuteStream(rt *runtime.Runtime, sess *session.Session, agentSpec, message string) (*Response, error) {
	startTime := time.Now()

	e.logger.Debug("Executing stream", "session_id", sess.ID, "message_length", len(message))

	// Add user message to session
	sess.Messages = append(sess.Messages, session.UserMessage(agentSpec, message))

	// Start streaming execution
	ctx := context.Background()
	eventStream := rt.RunStream(ctx, sess)

	// Collect events and final response
	var events []runtime.Event
	var finalContent string
	var streamingContent strings.Builder
	toolCallCount := 0

	for event := range eventStream {
		events = append(events, event)

		e.logger.Debug("Processing event", "event_type", fmt.Sprintf("%T", event))

		switch evt := event.(type) {
		case *runtime.AgentChoiceEvent:
			// This contains the streaming content from the model
			streamingContent.WriteString(evt.Choice.Delta.Content)
			e.logger.Debug("Agent choice event", "delta_length", len(evt.Choice.Delta.Content), "delta_content", evt.Choice.Delta.Content)

		case *runtime.ToolCallEvent:
			toolCallCount++
			e.logger.Debug("Tool call event", "tool_name", evt.ToolCall.Function.Name)

		case *runtime.ToolCallResponseEvent:
			e.logger.Debug("Tool response event", "tool_name", evt.ToolCall.Function.Name, "response_length", len(evt.Response))

		case *runtime.ToolCallConfirmationEvent:
			e.logger.Debug("Tool call confirmation event", "tool_name", evt.ToolCall.Function.Name)

		case *runtime.ErrorEvent:
			e.logger.Error("Runtime error event", "error", evt.Error)
			return nil, fmt.Errorf("runtime execution error: %w", evt.Error)
		default:
			e.logger.Debug("Unknown event type", "event_type", fmt.Sprintf("%T", event))
		}
	}

	duration := time.Since(startTime)

	// Use streaming content if available, fallback to final content
	content := finalContent
	if content == "" && streamingContent.Len() > 0 {
		content = streamingContent.String()
		e.logger.Debug("Using streaming content as final content", "content_length", len(content))
	}

	// Build response with metadata
	response := &Response{
		Content: content,
		Events:  events,
		Metadata: map[string]interface{}{
			"duration_ms":    duration.Milliseconds(),
			"tool_calls":     toolCallCount,
			"event_count":    len(events),
			"session_id":     sess.ID,
			"message_length": len(message),
		},
	}

	e.logger.Debug("Stream execution completed",
		"session_id", sess.ID,
		"duration_ms", duration.Milliseconds(),
		"tool_calls", toolCallCount,
		"events", len(events),
		"final_content_length", len(finalContent),
		"streaming_content_length", streamingContent.Len(),
		"response_content_length", len(content),
		"response_content_preview", func() string {
			if len(content) > 100 {
				return content[:100] + "..."
			}
			return content
		}())

	return response, nil
}

// CleanupRuntime cleans up runtime resources
func (e *Executor) CleanupRuntime(rt *runtime.Runtime) error {
	if rt == nil {
		return nil
	}

	// Stop tool sets
	if err := rt.Team().StopToolSets(); err != nil {
		e.logger.Warn("Error stopping tool sets during cleanup", "error", err)
		return fmt.Errorf("stopping tool sets: %w", err)
	}

	e.logger.Debug("Runtime cleaned up successfully")
	return nil
}
