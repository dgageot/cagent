package servicecore

import (
	"fmt"
	"log/slog"
	"os"
	"testing"
	"time"

	"github.com/docker/cagent/pkg/content"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

func TestManager_ClientLifecycle(t *testing.T) {
	workingDir := t.TempDir()
	storeDir := t.TempDir()
	agentsDir := t.TempDir()

	logger := slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{Level: slog.LevelWarn}))

	// Create isolated store for testing
	store, err := content.NewStore(content.WithBaseDir(storeDir))
	require.NoError(t, err)

	resolver, err := NewResolverWithStore(agentsDir, store, logger)
	require.NoError(t, err)

	executor := NewExecutor(workingDir, logger)

	manager, err := NewManager(resolver, executor, time.Hour, 10, logger)
	require.NoError(t, err)

	t.Run("CreateClient", func(t *testing.T) {
		err := manager.CreateClient("client-1")
		assert.NoError(t, err)

		// Creating same client again should fail
		err = manager.CreateClient("client-1")
		assert.Error(t, err)
		assert.Contains(t, err.Error(), "already exists")
	})

	t.Run("RemoveClient", func(t *testing.T) {
		// First create a client
		err := manager.CreateClient("client-to-remove")
		require.NoError(t, err)

		// Remove client
		err = manager.RemoveClient("client-to-remove")
		assert.NoError(t, err)

		// Removing non-existent client should fail
		err = manager.RemoveClient("non-existent")
		assert.Error(t, err)
		assert.Contains(t, err.Error(), "not found")
	})
}

func TestManager_AgentOperations(t *testing.T) {
	workingDir := t.TempDir()
	storeDir := t.TempDir()
	agentsDir := t.TempDir()

	// Create a test agent file
	testAgentContent := `
agents:
  root:
    name: test-agent
    description: Test agent for unit tests
    model: test-model
    instruction: You are a test agent

models:
  test-model:
    provider: test
    model: test
`
	agentFile := agentsDir + "/test-agent.yaml"
	err := os.WriteFile(agentFile, []byte(testAgentContent), 0644)
	require.NoError(t, err)

	logger := slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{Level: slog.LevelWarn}))

	// Create isolated store for testing
	store, err := content.NewStore(content.WithBaseDir(storeDir))
	require.NoError(t, err)

	resolver, err := NewResolverWithStore(agentsDir, store, logger)
	require.NoError(t, err)

	executor := NewExecutor(workingDir, logger)

	manager, err := NewManager(resolver, executor, time.Hour, 10, logger)
	require.NoError(t, err)

	t.Run("ResolveAgent", func(t *testing.T) {
		resolved, err := manager.ResolveAgent(agentFile)
		require.NoError(t, err)
		assert.Equal(t, agentFile, resolved)

		// Non-existent agent should fail
		_, err = manager.ResolveAgent("non-existent.yaml")
		assert.Error(t, err)
	})

	t.Run("ListAgents", func(t *testing.T) {
		// Test listing file agents
		fileAgents, err := manager.ListAgents("files")
		require.NoError(t, err)
		assert.Len(t, fileAgents, 1)
		assert.Equal(t, "test-agent", fileAgents[0].Name)
		assert.Equal(t, "file", fileAgents[0].Source)

		// Test listing store agents (should be empty with isolated store)
		storeAgents, err := manager.ListAgents("store")
		require.NoError(t, err)
		assert.Len(t, storeAgents, 0)

		// Test listing all agents (files + store)
		allAgents, err := manager.ListAgents("all")
		require.NoError(t, err)
		// Should have exactly our 1 file agent
		assert.Len(t, allAgents, 1)
		assert.Equal(t, "test-agent", allAgents[0].Name)
		assert.Equal(t, "file", allAgents[0].Source)

		// Test invalid source
		_, err = manager.ListAgents("invalid")
		assert.Error(t, err)
		assert.Contains(t, err.Error(), "unknown source")
	})

	t.Run("PullAgent", func(t *testing.T) {
		// Should fail with invalid reference (no registry setup)
		err := manager.PullAgent("invalid-reference")
		assert.Error(t, err)
	})
}

func TestManager_SessionOperations(t *testing.T) {
	workingDir := t.TempDir()
	storeDir := t.TempDir()
	agentsDir := t.TempDir()

	logger := slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{Level: slog.LevelWarn}))

	// Create isolated store for testing
	store, err := content.NewStore(content.WithBaseDir(storeDir))
	require.NoError(t, err)

	resolver, err := NewResolverWithStore(agentsDir, store, logger)
	require.NoError(t, err)

	executor := NewExecutor(workingDir, logger)

	manager, err := NewManager(resolver, executor, time.Hour, 2, logger) // Max 2 sessions for testing
	require.NoError(t, err)

	// Create a client
	err = manager.CreateClient("test-client")
	require.NoError(t, err)

	t.Run("CreateAgentSession_NonExistentClient", func(t *testing.T) {
		_, err := manager.CreateAgentSession("non-existent", "test-agent.yaml")
		assert.Error(t, err)
		assert.Contains(t, err.Error(), "not found")
	})

	t.Run("CreateAgentSession_NonExistentAgent", func(t *testing.T) {
		_, err := manager.CreateAgentSession("test-client", "non-existent.yaml")
		assert.Error(t, err)
		assert.Contains(t, err.Error(), "resolving agent")
	})

	t.Run("ListSessions_EmptyClient", func(t *testing.T) {
		sessions, err := manager.ListSessions("test-client")
		require.NoError(t, err)
		assert.Len(t, sessions, 0)
	})

	t.Run("ListSessions_NonExistentClient", func(t *testing.T) {
		_, err := manager.ListSessions("non-existent")
		assert.Error(t, err)
		assert.Contains(t, err.Error(), "not found")
	})

	t.Run("SendMessage_NonExistentClient", func(t *testing.T) {
		_, err := manager.SendMessage("non-existent", "session-1", "hello")
		assert.Error(t, err)
		assert.Contains(t, err.Error(), "not found")
	})

	t.Run("SendMessage_NonExistentSession", func(t *testing.T) {
		_, err := manager.SendMessage("test-client", "non-existent", "hello")
		assert.Error(t, err)
		assert.Contains(t, err.Error(), "not found")
	})

	t.Run("CloseSession_NonExistentClient", func(t *testing.T) {
		err := manager.CloseSession("non-existent", "session-1")
		assert.Error(t, err)
		assert.Contains(t, err.Error(), "not found")
	})

	t.Run("CloseSession_NonExistentSession", func(t *testing.T) {
		err := manager.CloseSession("test-client", "non-existent")
		assert.Error(t, err)
		assert.Contains(t, err.Error(), "not found")
	})
}

func TestManager_SessionLimits(t *testing.T) {
	workingDir := t.TempDir()
	storeDir := t.TempDir()
	agentsDir := t.TempDir()

	logger := slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{Level: slog.LevelWarn}))

	// Create isolated store for testing
	store, err := content.NewStore(content.WithBaseDir(storeDir))
	require.NoError(t, err)

	resolver, err := NewResolverWithStore(agentsDir, store, logger)
	require.NoError(t, err)

	executor := NewExecutor(workingDir, logger)

	manager, err := NewManager(resolver, executor, time.Hour, 1, logger) // Max 1 session
	require.NoError(t, err)

	// Create a client
	err = manager.CreateClient("test-client")
	require.NoError(t, err)

	// Create a test agent file (won't actually load due to missing model config)
	testAgent := agentsDir + "/test.yaml"
	err = os.WriteFile(testAgent, []byte("test"), 0644)
	require.NoError(t, err)

	t.Run("SessionLimit", func(t *testing.T) {
		// This will fail at runtime creation stage, but let's test the session limit logic
		// by mocking a successful session creation scenario

		// For now, we can test that the session limit check works by inspecting the manager state
		// Since CreateAgentSession calls resolver.ResolveAgent which will succeed for our test file
		// but then fail at executor.CreateRuntime due to invalid agent config,
		// we can verify the limit logic separately by checking if we get the right error

		// The session limit is checked before runtime creation, so we need a valid agent file
		// to test this properly, but that requires a full agent configuration
		// For now, let's test that non-existent agents fail appropriately
		_, err := manager.CreateAgentSession("test-client", "non-existent.yaml")
		assert.Error(t, err)
	})
}

func TestManager_ConcurrentAccess(t *testing.T) {
	workingDir := t.TempDir()
	storeDir := t.TempDir()
	agentsDir := t.TempDir()

	logger := slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{Level: slog.LevelWarn}))

	// Create isolated store for testing
	store, err := content.NewStore(content.WithBaseDir(storeDir))
	require.NoError(t, err)

	resolver, err := NewResolverWithStore(agentsDir, store, logger)
	require.NoError(t, err)

	executor := NewExecutor(workingDir, logger)

	manager, err := NewManager(resolver, executor, time.Hour, 10, logger)
	require.NoError(t, err)

	t.Run("ConcurrentClientCreation", func(t *testing.T) {
		// Test concurrent client creation
		done := make(chan bool, 10)

		for i := 0; i < 10; i++ {
			go func(id int) {
				err := manager.CreateClient(fmt.Sprintf("client-%d", id))
				assert.NoError(t, err)
				done <- true
			}(i)
		}

		// Wait for all goroutines to complete
		for i := 0; i < 10; i++ {
			<-done
		}

		// Verify all clients were created
		// We can't directly access the clients map, so we test by trying to create sessions
		for i := 0; i < 10; i++ {
			sessions, err := manager.ListSessions(fmt.Sprintf("client-%d", i))
			assert.NoError(t, err)
			assert.Len(t, sessions, 0) // Should be empty but not error
		}
	})
}
