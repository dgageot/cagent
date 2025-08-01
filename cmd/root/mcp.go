package root

import (
	"context"
	"fmt"
	"log/slog"
	"os"
	"os/signal"
	"syscall"
	"time"

	"github.com/docker/cagent/pkg/mcpserver"
	"github.com/docker/cagent/pkg/servicecore"
	"github.com/spf13/cobra"
)

var (
	agentsDir      string
	maxSessions    int
	sessionTimeout time.Duration
	port           string
	basePath       string
)

// NewMCPCmd creates the MCP server command
func NewMCPCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "mcp server",
		Short: "Start cagent in MCP server mode",
		Long: `Start cagent as an MCP (Model Context Protocol) server.
This allows external clients like Claude Code to programmatically invoke cagent agents
and maintain conversational sessions.`,
		RunE: runMCPCommand,
	}

	cmd.Flags().StringVar(&agentsDir, "agents-dir", "", "Directory containing agent configs (defaults to current directory)")
	cmd.Flags().IntVar(&maxSessions, "max-sessions", 100, "Maximum concurrent sessions")
	cmd.Flags().DurationVar(&sessionTimeout, "session-timeout", time.Hour, "Session timeout duration")
	cmd.Flags().StringVar(&port, "port", "8080", "Port for MCP SSE server")
	cmd.Flags().StringVar(&basePath, "path", "/mcp", "Base path for MCP endpoints (e.g., /mcp, /foo/bar)")
	cmd.Flags().BoolVar(&debugMode, "debug", false, "Enable debug logging")

	return cmd
}

func runMCPCommand(cmd *cobra.Command, args []string) error {
	// Initialize logger with appropriate level
	handlerOpts := &slog.HandlerOptions{
		Level: slog.LevelInfo,
	}
	if debugMode {
		handlerOpts.Level = slog.LevelDebug
	}
	logger := slog.New(slog.NewTextHandler(os.Stderr, handlerOpts))

	if debugMode {
		logger.Info("Debug mode enabled")
	}

	cwd, err := os.Getwd()
	if err != nil {
		return fmt.Errorf("getting current working directory: %w", err)
	}

	// Default agents directory to current working directory if not specified
	resolvedAgentsDir := agentsDir
	if resolvedAgentsDir == "" {
		resolvedAgentsDir = cwd
		logger.Info("Using current directory as agents root", "path", resolvedAgentsDir)
	}

	// Create servicecore manager
	resolver, err := servicecore.NewResolver(resolvedAgentsDir, logger)
	if err != nil {
		return fmt.Errorf("creating resolver: %w", err)
	}

	executor := servicecore.NewExecutor(cwd, logger)

	serviceCore, err := servicecore.NewManager(resolver, executor, sessionTimeout, maxSessions, logger)
	if err != nil {
		return fmt.Errorf("creating servicecore manager: %w", err)
	}

	// Create MCP server using servicecore
	mcpServer := mcpserver.NewMCPServer(serviceCore, logger, basePath)

	// Set up context with cancellation
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	// Set up signal handling for graceful shutdown
	sigChan := make(chan os.Signal, 1)
	signal.Notify(sigChan, syscall.SIGINT, syscall.SIGTERM)

	go func() {
		<-sigChan
		logger.Info("Received shutdown signal, stopping MCP server")
		cancel()
	}()

	// Build the complete endpoint URLs for client connection
	sseEndpoint := fmt.Sprintf("http://localhost:%s%s/sse", port, basePath)
	messageEndpoint := fmt.Sprintf("http://localhost:%s%s/message", port, basePath)

	logger.Info("MCP SSE server starting",
		"agents_dir", resolvedAgentsDir,
		"max_sessions", maxSessions,
		"timeout", sessionTimeout,
		"port", port,
		"base_path", basePath)

	fmt.Printf("\n🚀 MCP Server Ready!\n")
	fmt.Printf("📡 SSE Endpoint:     %s\n", sseEndpoint)
	fmt.Printf("💬 Message Endpoint: %s\n", messageEndpoint)
	fmt.Printf("📁 Agents Directory: %s\n", resolvedAgentsDir)
	fmt.Printf("\nMCP clients should connect to: %s\n\n", sseEndpoint)

	// Start MCP SSE server
	if err := mcpServer.Start(ctx, port); err != nil {
		return fmt.Errorf("starting MCP SSE server: %w", err)
	}

	logger.Info("MCP server stopped")
	return nil
}
