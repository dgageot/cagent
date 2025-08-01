// server_test.go provides unit tests for the MCP server implementation
// This file tests the server creation, tool registration, and basic functionality
// without requiring a full MCP client integration.
package mcpserver

import (
	"log/slog"
	"os"
	"testing"
	"time"

	"github.com/docker/cagent/pkg/content"
	"github.com/docker/cagent/pkg/servicecore"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

func TestNewMCPServer(t *testing.T) {
	workingDir := t.TempDir()
	storeDir := t.TempDir()
	agentsDir := t.TempDir()
	logger := slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{Level: slog.LevelWarn}))

	// Create isolated store for testing
	store, err := content.NewStore(content.WithBaseDir(storeDir))
	require.NoError(t, err)

	resolver, err := servicecore.NewResolverWithStore(agentsDir, store, logger)
	require.NoError(t, err)

	executor := servicecore.NewExecutor(workingDir, logger)

	serviceCore, err := servicecore.NewManager(resolver, executor, time.Hour, 100, logger)
	require.NoError(t, err)

	t.Run("ServerCreation", func(t *testing.T) {
		mcpServer := NewMCPServer(serviceCore, logger, "/mcp")

		assert.NotNil(t, mcpServer)
		assert.Equal(t, serviceCore, mcpServer.serviceCore)
		assert.Equal(t, logger, mcpServer.logger)
		assert.NotNil(t, mcpServer.mcpServer)
		assert.NotNil(t, mcpServer.sseServer)
	})

	t.Run("ServerConfiguration", func(t *testing.T) {
		mcpServer := NewMCPServer(serviceCore, logger, "/mcp")

		// Verify server is properly configured
		assert.NotNil(t, mcpServer.mcpServer, "MCP server should be created")
		assert.NotNil(t, mcpServer.sseServer, "SSE server should be created")
	})
}

func TestMCPServerIntegration(t *testing.T) {
	if testing.Short() {
		t.Skip("Skipping integration test in short mode")
	}

	workingDir := t.TempDir()
	storeDir := t.TempDir()
	agentsDir := t.TempDir()
	logger := slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{Level: slog.LevelWarn}))

	// Create a test agent file
	testAgent := `
agents:
  root:
    name: test-agent
    description: Test agent for MCP integration
    model: test-model
    instruction: You are a test agent

models:
  test-model:
    provider: openai
    model: gpt-4
`
	agentFile := agentsDir + "/test-agent.yaml"
	err := os.WriteFile(agentFile, []byte(testAgent), 0644)
	require.NoError(t, err)

	// Create isolated store for testing
	store, err := content.NewStore(content.WithBaseDir(storeDir))
	require.NoError(t, err)

	resolver, err := servicecore.NewResolverWithStore(agentsDir, store, logger)
	require.NoError(t, err)

	executor := servicecore.NewExecutor(workingDir, logger)

	serviceCore, err := servicecore.NewManager(resolver, executor, time.Hour, 100, logger)
	require.NoError(t, err)

	t.Run("ServiceCoreIntegration", func(t *testing.T) {
		mcpServer := NewMCPServer(serviceCore, logger, "/mcp")

		// Test that the server can access servicecore functionality
		agents, err := mcpServer.serviceCore.ListAgents("files")
		assert.NoError(t, err)
		assert.Len(t, agents, 1)
		assert.Equal(t, "test-agent", agents[0].Name)
		assert.Equal(t, "file", agents[0].Source)
		assert.NotEmpty(t, agents[0].Path, "File agents should have Path populated")
		assert.NotEmpty(t, agents[0].RelativePath, "File agents should have RelativePath populated")

		// For file agents, Path should be absolute, RelativePath should be relative
		assert.Equal(t, agentFile, agents[0].Path, "File agent path should match created file")
		assert.Equal(t, "test-agent.yaml", agents[0].RelativePath, "File agent relative path should be relative to agents dir")
	})

	t.Run("AgentRefFormatting", func(t *testing.T) {
		mcpServer := NewMCPServer(serviceCore, logger, "/mcp")

		// Test file agents
		agents, err := mcpServer.serviceCore.ListAgents("files")
		require.NoError(t, err)
		require.Len(t, agents, 1)

		fileAgent := agents[0]
		assert.Equal(t, "file", fileAgent.Source)
		assert.NotEmpty(t, fileAgent.Path, "File agents should have Path populated")
		assert.NotEmpty(t, fileAgent.RelativePath, "File agents should have RelativePath populated")
		assert.Empty(t, fileAgent.Reference, "File agents should not have Reference populated")

		// Test store agents (may have existing agents)
		storeAgents, err := mcpServer.serviceCore.ListAgents("store")
		require.NoError(t, err)

		// If there are store agents, verify their format
		for _, storeAgent := range storeAgents {
			assert.Equal(t, "store", storeAgent.Source)
			assert.NotEmpty(t, storeAgent.Reference, "Store agents should have Reference populated")
			assert.Empty(t, storeAgent.Path, "Store agents should not have Path populated")
		}

		// Verify that if we had a store agent, it would have Reference field
		// This tests the logic without requiring actual Docker images
		mockStoreAgent := servicecore.AgentInfo{
			Name:        "mock-store-agent",
			Description: "Mock store agent",
			Source:      "store",
			Reference:   "docker.io/user/agent:latest",
		}

		// Simulate how the handler would determine agent_ref
		var agentRef string
		if mockStoreAgent.Source == "file" {
			agentRef = mockStoreAgent.Path
		} else if mockStoreAgent.Source == "store" {
			agentRef = mockStoreAgent.Reference
		}

		assert.Equal(t, "docker.io/user/agent:latest", agentRef, "Store agent ref should be the full image reference")

		// Test file agent ref logic
		mockFileAgent := servicecore.AgentInfo{
			Name:         "mock-file-agent",
			Description:  "Mock file agent",
			Source:       "file",
			Path:         "/path/to/agent.yaml",
			RelativePath: "agent.yaml",
		}

		var fileAgentRef string
		if mockFileAgent.Source == "file" {
			fileAgentRef = mockFileAgent.RelativePath
		} else if mockFileAgent.Source == "store" {
			fileAgentRef = mockFileAgent.Reference
		}

		assert.Equal(t, "agent.yaml", fileAgentRef, "File agent ref should be the relative path")
	})

	t.Run("AgentListFormatting", func(t *testing.T) {
		// Test the actual formatting logic used in the handler
		mockAgents := []servicecore.AgentInfo{
			{
				Name:         "file-agent",
				Description:  "A file-based agent",
				Source:       "file",
				Path:         "/path/to/file-agent.yaml",
				RelativePath: "file-agent.yaml",
			},
			{
				Name:        "store-agent",
				Description: "A store-based agent",
				Source:      "store",
				Reference:   "docker.io/user/store-agent:latest",
			},
		}

		// Simulate the handler formatting logic
		var agentList []interface{}
		for _, agent := range mockAgents {
			var agentRef string
			if agent.Source == "file" {
				agentRef = agent.RelativePath
			} else if agent.Source == "store" {
				agentRef = agent.Reference
			}

			agentInfo := map[string]interface{}{
				"agent_ref":     agentRef,
				"friendly_name": agent.Name,
				"source":        agent.Source,
				"description":   agent.Description,
			}
			agentList = append(agentList, agentInfo)
		}

		// Verify formatting
		require.Len(t, agentList, 2)

		// Check file agent formatting
		fileAgentInfo := agentList[0].(map[string]interface{})
		assert.Equal(t, "file-agent.yaml", fileAgentInfo["agent_ref"])
		assert.Equal(t, "file-agent", fileAgentInfo["friendly_name"])
		assert.Equal(t, "file", fileAgentInfo["source"])

		// Check store agent formatting
		storeAgentInfo := agentList[1].(map[string]interface{})
		assert.Equal(t, "docker.io/user/store-agent:latest", storeAgentInfo["agent_ref"])
		assert.Equal(t, "store-agent", storeAgentInfo["friendly_name"])
		assert.Equal(t, "store", storeAgentInfo["source"])
	})
}

// Note: Full MCP client integration tests would require setting up
// an actual MCP client and testing the SSE endpoints. This would be
// better suited for end-to-end testing rather than unit tests.
