package new

import (
	"bufio"
	"fmt"
	"log/slog"
	"os"
	"strings"

	"github.com/fatih/color"
	"github.com/spf13/cobra"

	"github.com/docker/cagent/internal/creator"
	"github.com/docker/cagent/pkg/runtime"
)

// Cmd creates a new command to create a new agent configuration
func Cmd() *cobra.Command {
	return &cobra.Command{
		Use:   "new",
		Short: "Create a new agent configuration",
		Long:  `Create a new agent configuration by asking questions and generating a YAML file`,
		RunE:  newAgentCommand,
	}
}

func newAgentCommand(cmd *cobra.Command, args []string) error {
	ctx := cmd.Context()
	cwd, err := os.Getwd()
	if err != nil {
		return err
	}

	prompt := ""
	if len(args) > 0 {
		prompt = strings.Join(args, " ")
	} else {
		reader := bufio.NewReader(os.Stdin)

		fmt.Print("What should your agent do? (describe its purpose): ")
		var err error
		prompt, err = reader.ReadString('\n')
		if err != nil {
			return fmt.Errorf("failed to read purpose: %w", err)
		}
		prompt = strings.TrimSpace(prompt)
	}

	logger := slog.Default()
	out, err := creator.StreamCreateAgent(ctx, cwd, prompt, logger)
	if err != nil {
		return err
	}

	yellow := color.New(color.FgYellow).SprintfFunc()
	green := color.New(color.FgGreen).SprintfFunc()

	for event := range out {
		switch e := event.(type) {
		case *runtime.AgentChoiceEvent:
			fmt.Printf("%s", e.Choice.Delta.Content)
		case *runtime.ToolCallEvent:
			fmt.Printf("%s", yellow("\n%s(%s)\n", e.ToolCall.Function.Name, e.ToolCall.Function.Arguments))
		case *runtime.ToolCallResponseEvent:
			fmt.Printf("%s", green("done(%s)\n", e.ToolCall.Function.Name))
		case *runtime.ErrorEvent:
			fmt.Printf("%s\n", e.Error)
		}
	}

	return nil
}
