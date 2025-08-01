package root

import (
	"bufio"
	"bytes"
	"context"
	"fmt"
	"io"
	"log/slog"
	"os"
	"strings"

	"github.com/fatih/color"
	"github.com/spf13/cobra"

	"github.com/docker/cagent/pkg/content"
	"github.com/docker/cagent/pkg/evaluation"
	"github.com/docker/cagent/pkg/loader"
	"github.com/docker/cagent/pkg/runtime"
	"github.com/docker/cagent/pkg/session"
)

// NewRunCmd creates a new run command
func NewRunCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "run <agent-name> [message|-]",
		Short: "Run an agent",
		Long:  `Run an agent with the specified configuration and prompt`,
		Example: `  cagent run ./agent.yaml
  cagent run ./team.yaml --agent root
  cagent run ./echo.yaml "ECHO"
  echo "ECHO" | cagent run ./echo.yaml -`,
		Args: cobra.RangeArgs(1, 2),
		RunE: runAgentCommand,
	}

	cmd.PersistentFlags().StringVarP(&agentName, "agent", "a", "root", "Name of the agent to run")
	cmd.PersistentFlags().BoolVarP(&debugMode, "debug", "d", false, "Enable debug logging")
	cmd.PersistentFlags().StringSliceVar(&runConfig.EnvFiles, "env-from-file", nil, "Set environment variables from file")
	addGatewayFlags(cmd)

	return cmd
}

func runAgentCommand(cmd *cobra.Command, args []string) error {
	ctx := context.Background()
	agentFilename := args[0]
	cwd, err := os.Getwd()
	if err != nil {
		return err
	}

	logLevel := slog.LevelInfo
	if debugMode {
		logLevel = slog.LevelDebug
	}

	logger := slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{
		Level: logLevel,
	}))

	logger.Debug("Starting agent", "agent", agentName, "debug_mode", debugMode)

	if !fileExists(agentFilename) {
		a, err := fromStore(agentFilename)
		if err != nil {
			return err
		}
		tmpFile, err := os.CreateTemp("", "agentfile-*.yaml")
		if err != nil {
			return err
		}
		defer os.Remove(tmpFile.Name())

		if _, err := tmpFile.WriteString(a); err != nil {
			tmpFile.Close()
			return err
		}
		if err := tmpFile.Close(); err != nil {
			return err
		}
		agentFilename = tmpFile.Name()
	}

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
	sess := session.New(cwd, logger)

	blue := color.New(color.FgBlue).SprintfFunc()
	yellow := color.New(color.FgYellow).SprintfFunc()
	green := color.New(color.FgGreen).SprintfFunc()

	// If the last received event was an error, return it. That way the exit code
	// will be non-zero if the agent failed.
	var lastErr error

	oneLoop := func(text string, scannerConfirmations *bufio.Scanner) error {
		userInput := strings.TrimSpace(text)
		if userInput == "" {
			return nil
		}

		handled, err := runUserCommand(userInput, sess)
		if err != nil {
			return err
		}

		if handled {
			return nil
		}

		sess.Messages = append(sess.Messages, session.UserMessage(agentFilename, userInput))

		first := false
		for event := range rt.RunStream(ctx, sess) {
			if !first {
				fmt.Printf("%s", blue("[%s]: ", rt.CurrentAgent().Name()))
				first = true
			}
			lastErr = nil
			switch e := event.(type) {
			case *runtime.AgentChoiceEvent:
				fmt.Printf("%s", e.Choice.Delta.Content)
			case *runtime.ToolCallConfirmationEvent:
				fmt.Printf("%s", yellow("\n%s(%s)\n", e.ToolCall.Function.Name, e.ToolCall.Function.Arguments))
				fmt.Println("\nCan I run this tool? (y/a/n)")
				scannerConfirmations.Scan()
				text := scannerConfirmations.Text()
				switch text {
				case "y":
					rt.Resume(ctx, string(runtime.ResumeTypeApprove))
				case "a":
					sess.ToolsApproved = true
					rt.Resume(ctx, string(runtime.ResumeTypeApproveSession))
				case "n":
					rt.Resume(ctx, string(runtime.ResumeTypeReject))
				}
			case *runtime.ToolCallEvent:
				fmt.Printf("%s", yellow("\n%s(%s)\n", e.ToolCall.Function.Name, e.ToolCall.Function.Arguments))
			case *runtime.ToolCallResponseEvent:
				fmt.Printf("%s", green("done(%s)\n", e.ToolCall.Function.Name))
			case *runtime.ErrorEvent:
				fmt.Printf("%s\n", e.Error)
				lastErr = e.Error
			}
		}
		fmt.Println()
		return nil
	}

	if len(args) == 2 {
		if args[1] == "-" {
			buf, err := io.ReadAll(os.Stdin)
			if err != nil {
				return fmt.Errorf("failed to read from stdin: %w", err)
			}

			if err := oneLoop(string(buf), bufio.NewScanner(os.Stdin)); err != nil {
				return err
			}
		} else {
			if err := oneLoop(args[1], bufio.NewScanner(os.Stdin)); err != nil {
				return err
			}
		}
	} else {
		fmt.Println(blue("\nEnter your messages (Ctrl+C to exit):"))
		scanner := bufio.NewScanner(os.Stdin)
		for {
			fmt.Print(blue("> "))

			if !scanner.Scan() {
				break
			}

			if err := oneLoop(scanner.Text(), scanner); err != nil {
				return err
			}
		}

		if err := scanner.Err(); err != nil {
			return err
		}
	}

	return lastErr
}

func runUserCommand(userInput string, sess *session.Session) (bool, error) {
	yellow := color.New(color.FgYellow).SprintfFunc()
	switch userInput {
	case "/exit":
		os.Exit(0)
	case "/eval":
		err := evaluation.Save(sess)
		if err == nil {
			fmt.Printf("%s\n", yellow("Evaluation saved"))
			return true, err
		}
		return true, nil
	case "/reset":
		sess.Messages = []session.Message{}
		return true, nil
	}

	return false, nil
}

func fileExists(path string) bool {
	_, err := os.Stat(path)
	exists := err == nil
	return exists
}

func fromStore(reference string) (string, error) {
	store, err := content.NewStore()
	if err != nil {
		return "", err
	}

	img, err := store.GetArtifactImage(reference)
	if err != nil {
		return "", err
	}

	layers, err := img.Layers()
	if err != nil {
		return "", err
	}

	var buf bytes.Buffer
	layer := layers[0]
	b, err := layer.Uncompressed()
	if err != nil {
		return "", err
	}

	_, err = io.Copy(&buf, b)
	if err != nil {
		return "", err
	}
	b.Close()

	return buf.String(), nil
}
