// manager.go implements the ServiceManager interface as the central orchestrator
// for multi-tenant agent operations in cagent's MCP mode.
//
// This file serves as the primary coordination layer that:
// 1. Manages client lifecycle with proper isolation between MCP clients
// 2. Orchestrates agent resolution, runtime creation, and session management
// 3. Enforces security boundaries and resource limits per client
// 4. Provides thread-safe operations for concurrent client access
//
// Key Components:
// - Client management: Creates, tracks, and cleans up client sessions
// - Agent operations: Delegates to Resolver for secure agent specification handling
// - Session lifecycle: Coordinates Executor for runtime creation and message processing
// - Resource limits: Enforces maximum sessions per client and proper cleanup
//
// Security Considerations:
// - All operations are client-scoped to prevent cross-client access
// - Agent resolution is restricted to configured root directories
// - Proper resource cleanup prevents memory leaks and zombie processes
//
// The Manager acts as the single entry point for both MCP and future HTTP transports,
// ensuring consistent behavior and security across all access methods.
package servicecore

import (
	"context"
	"fmt"
	"log/slog"
	"sync"
	"time"
)

// Manager implements ServiceManager with multi-tenant client and session management
type Manager struct {
	clients     map[string]*Client
	store       Store
	resolver    *Resolver
	executor    *Executor
	timeout     time.Duration
	maxSessions int
	mutex       sync.RWMutex
	logger      *slog.Logger
}

// NewManager creates a new ServiceManager
func NewManager(resolver *Resolver, executor *Executor, timeout time.Duration, maxSessions int, logger *slog.Logger) (ServiceManager, error) {
	// Initialize SQLite store (for future session persistence)
	// For now, we'll use nil store since we're managing sessions in memory
	var store Store
	// TODO: Initialize actual store when session persistence is needed
	// store, err := NewSQLiteStore(":memory:", logger)
	// if err != nil {
	//     return nil, fmt.Errorf("creating store: %w", err)
	// }

	return &Manager{
		clients:     make(map[string]*Client),
		store:       store,
		resolver:    resolver,
		executor:    executor,
		timeout:     timeout,
		maxSessions: maxSessions,
		logger:      logger,
	}, nil
}

// CreateClient creates a new client session
func (m *Manager) CreateClient(clientID string) error {
	m.mutex.Lock()
	defer m.mutex.Unlock()

	if _, exists := m.clients[clientID]; exists {
		return fmt.Errorf("client %s already exists", clientID)
	}

	m.clients[clientID] = &Client{
		ID:            clientID,
		AgentSessions: make(map[string]*AgentSession),
		Created:       time.Now(),
		LastUsed:      time.Now(),
	}

	m.logger.Info("Client created", "client_id", clientID)
	return nil
}

// RemoveClient removes a client and cleans up all associated sessions
func (m *Manager) RemoveClient(clientID string) error {
	m.mutex.Lock()
	defer m.mutex.Unlock()

	client, exists := m.clients[clientID]
	if !exists {
		return fmt.Errorf("client %s not found", clientID)
	}

	// Clean up all agent sessions for this client
	for sessionID := range client.AgentSessions {
		if err := m.closeSessionUnsafe(clientID, sessionID); err != nil {
			m.logger.Warn("Error closing session during client cleanup",
				"client_id", clientID, "session_id", sessionID, "error", err)
		}
	}

	delete(m.clients, clientID)
	m.logger.Info("Client removed", "client_id", clientID)
	return nil
}

// ResolveAgent resolves an agent specification to a file path
func (m *Manager) ResolveAgent(agentSpec string) (string, error) {
	return m.resolver.ResolveAgent(agentSpec)
}

// ListAgents lists available agents from files and store
func (m *Manager) ListAgents(source string) ([]AgentInfo, error) {
	switch source {
	case "files":
		return m.resolver.ListFileAgents()
	case "store":
		return m.resolver.ListStoreAgents()
	case "all", "":
		fileAgents, err := m.resolver.ListFileAgents()
		if err != nil {
			return nil, fmt.Errorf("listing file agents: %w", err)
		}
		storeAgents, err := m.resolver.ListStoreAgents()
		if err != nil {
			return nil, fmt.Errorf("listing store agents: %w", err)
		}
		return append(fileAgents, storeAgents...), nil
	default:
		return nil, fmt.Errorf("unknown source: %s (valid: files, store, all)", source)
	}
}

// PullAgent pulls an agent image from registry to local store
func (m *Manager) PullAgent(registryRef string) error {
	return m.resolver.PullAgent(registryRef)
}

// CreateAgentSession creates a new agent session for a client
func (m *Manager) CreateAgentSession(clientID, agentSpec string) (*AgentSession, error) {
	m.mutex.Lock()
	defer m.mutex.Unlock()

	client, exists := m.clients[clientID]
	if !exists {
		return nil, fmt.Errorf("client %s not found", clientID)
	}

	// Check session limits
	if len(client.AgentSessions) >= m.maxSessions {
		return nil, fmt.Errorf("maximum sessions (%d) reached for client %s", m.maxSessions, clientID)
	}

	// Resolve agent specification to file path
	agentPath, err := m.resolver.ResolveAgent(agentSpec)
	if err != nil {
		return nil, fmt.Errorf("resolving agent: %w", err)
	}

	// Create runtime and session
	rt, sess, err := m.executor.CreateRuntime(agentPath, "root", nil, "")
	if err != nil {
		return nil, fmt.Errorf("creating runtime: %w", err)
	}

	sessionID := sess.ID // Use the session ID from the session object

	agentSession := &AgentSession{
		ID:        sessionID,
		ClientID:  clientID,
		AgentSpec: agentSpec,
		Runtime:   rt,
		Session:   sess,
		Created:   time.Now(),
		LastUsed:  time.Now(),
	}

	client.AgentSessions[sessionID] = agentSession
	client.LastUsed = time.Now()

	m.logger.Info("Agent session created",
		"client_id", clientID, "session_id", sessionID, "agent_spec", agentSpec, "agent_path", agentPath)

	return agentSession, nil
}

// SendMessage sends a message to an agent session
func (m *Manager) SendMessage(clientID, sessionID, message string) (*Response, error) {
	m.mutex.RLock()
	client, exists := m.clients[clientID]
	if !exists {
		m.mutex.RUnlock()
		return nil, fmt.Errorf("client %s not found", clientID)
	}

	agentSession, exists := client.AgentSessions[sessionID]
	if !exists {
		m.mutex.RUnlock()
		return nil, fmt.Errorf("session %s not found for client %s", sessionID, clientID)
	}
	m.mutex.RUnlock()

	// Update last used time
	m.mutex.Lock()
	agentSession.LastUsed = time.Now()
	client.LastUsed = time.Now()
	m.mutex.Unlock()

	// Execute message using executor
	response, err := m.executor.ExecuteStream(agentSession.Runtime, agentSession.Session, agentSession.AgentSpec, message)
	if err != nil {
		return nil, fmt.Errorf("executing message: %w", err)
	}

	// Add client context to metadata
	response.Metadata["client_id"] = clientID

	m.logger.Debug("Message processed",
		"client_id", clientID, "session_id", sessionID, "message_length", len(message))

	return response, nil
}

// ListSessions lists all agent sessions for a client
func (m *Manager) ListSessions(clientID string) ([]*AgentSession, error) {
	m.mutex.RLock()
	defer m.mutex.RUnlock()

	client, exists := m.clients[clientID]
	if !exists {
		return nil, fmt.Errorf("client %s not found", clientID)
	}

	sessions := make([]*AgentSession, 0, len(client.AgentSessions))
	for _, session := range client.AgentSessions {
		sessions = append(sessions, session)
	}

	return sessions, nil
}

// CloseSession closes an agent session
func (m *Manager) CloseSession(clientID, sessionID string) error {
	m.mutex.Lock()
	defer m.mutex.Unlock()

	return m.closeSessionUnsafe(clientID, sessionID)
}

// closeSessionUnsafe closes a session without acquiring the mutex (internal use)
func (m *Manager) closeSessionUnsafe(clientID, sessionID string) error {
	client, exists := m.clients[clientID]
	if !exists {
		return fmt.Errorf("client %s not found", clientID)
	}

	agentSession, exists := client.AgentSessions[sessionID]
	if !exists {
		return fmt.Errorf("session %s not found for client %s", sessionID, clientID)
	}

	// Clean up runtime resources
	if agentSession.Runtime != nil {
		if err := m.executor.CleanupRuntime(agentSession.Runtime); err != nil {
			m.logger.Warn("Error cleaning up runtime", "error", err, "session_id", sessionID)
		}
	}

	delete(client.AgentSessions, sessionID)
	client.LastUsed = time.Now()

	m.logger.Info("Agent session closed",
		"client_id", clientID, "session_id", sessionID, "agent_spec", agentSession.AgentSpec)

	return nil
}

// GetSessionHistory retrieves conversation history for an agent session with optional pagination
func (m *Manager) GetSessionHistory(clientID, sessionID string, limit int) ([]SessionMessage, error) {
	m.mutex.RLock()
	defer m.mutex.RUnlock()

	client, exists := m.clients[clientID]
	if !exists {
		return nil, fmt.Errorf("client %s not found", clientID)
	}

	agentSession, exists := client.AgentSessions[sessionID]
	if !exists {
		return nil, fmt.Errorf("session %s not found for client %s", sessionID, clientID)
	}

	if agentSession.Session == nil {
		return []SessionMessage{}, nil
	}

	// Convert session messages to our format
	sessionMessages := agentSession.Session.Messages
	var result []SessionMessage

	// Apply limit if specified (0 means no limit)
	start := 0
	if limit > 0 && len(sessionMessages) > limit {
		start = len(sessionMessages) - limit
	}

	for i := start; i < len(sessionMessages); i++ {
		msg := sessionMessages[i]
		result = append(result, SessionMessage{
			AgentName: msg.AgentName,
			Role:      string(msg.Message.Role),
			Content:   msg.Message.Content,
			// Note: session.AgentMessage doesn't have timestamps,
			// we could enhance this in the future
		})
	}

	m.logger.Debug("Retrieved session history",
		"client_id", clientID, "session_id", sessionID,
		"total_messages", len(sessionMessages), "returned_messages", len(result))

	return result, nil
}

// GetSessionInfo retrieves detailed information about an agent session
func (m *Manager) GetSessionInfo(clientID, sessionID string) (*SessionInfo, error) {
	m.mutex.RLock()
	defer m.mutex.RUnlock()

	client, exists := m.clients[clientID]
	if !exists {
		return nil, fmt.Errorf("client %s not found", clientID)
	}

	agentSession, exists := client.AgentSessions[sessionID]
	if !exists {
		return nil, fmt.Errorf("session %s not found for client %s", sessionID, clientID)
	}

	messageCount := 0
	if agentSession.Session != nil {
		messageCount = len(agentSession.Session.Messages)
	}

	// Get agent name from runtime if available
	agentName := "unknown"
	if agentSession.Runtime != nil && agentSession.Runtime.CurrentAgent() != nil {
		agentName = agentSession.Runtime.CurrentAgent().Name()
	}

	metadata := map[string]interface{}{
		"agent_name":         agentName,
		"session_id":         agentSession.Session.ID,
		"session_created_at": agentSession.Session.CreatedAt,
	}

	// Add runtime info if available
	if agentSession.Runtime != nil && agentSession.Runtime.CurrentAgent() != nil {
		agent := agentSession.Runtime.CurrentAgent()
		metadata["agent_description"] = agent.Description()
		metadata["agent_instruction"] = agent.Instruction()

		// Add toolsets info
		toolsets := agent.ToolSets()
		metadata["toolsets_count"] = len(toolsets)

		// Get available tools from toolsets
		availableTools := []string{}
		for _, ts := range toolsets {
			tools, err := ts.Tools(context.Background())
			if err == nil {
				for _, tool := range tools {
					if tool.Function != nil {
						availableTools = append(availableTools, tool.Function.Name)
					}
				}
			}
		}
		metadata["available_tools"] = availableTools
	}

	sessionInfo := &SessionInfo{
		ID:           sessionID,
		ClientID:     clientID,
		AgentSpec:    agentSession.AgentSpec,
		Created:      agentSession.Created,
		LastUsed:     agentSession.LastUsed,
		MessageCount: messageCount,
		Metadata:     metadata,
	}

	m.logger.Debug("Retrieved session info",
		"client_id", clientID, "session_id", sessionID, "message_count", messageCount)

	return sessionInfo, nil
}
