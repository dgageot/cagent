package main

import (
	"context"
	"fmt"
	"log"
	"log/slog"
	"os"

	"github.com/docker/cagent/pkg/agent"
	latest "github.com/docker/cagent/pkg/config/v1"
	"github.com/docker/cagent/pkg/environment"
	"github.com/docker/cagent/pkg/model/provider/openai"
	"github.com/docker/cagent/pkg/runtime"
	"github.com/docker/cagent/pkg/session"
	"github.com/docker/cagent/pkg/team"
	"github.com/docker/cagent/pkg/tools/builtin"
)

func main() {
	ctx := context.Background()

	if err := run(ctx); err != nil {
		log.Fatal(err)
	}
}

func run(ctx context.Context) error {
	logger := slog.Default()
	cwd, err := os.Getwd()
	if err != nil {
		return err
	}

	llm, err := openai.NewClient(
		ctx,
		&latest.ModelConfig{
			Provider: "openai",
			Model:    "gpt-4o",
		},
		environment.NewDefaultProvider(logger),
		logger,
	)
	if err != nil {
		return err
	}

	agents := team.New(
		team.WithAgents(
			agent.New(
				"root",
				"You are an expert hacker",
				agent.WithModel(llm),
				agent.WithToolSets(builtin.NewShellTool()),
			),
		),
	)

	rt := runtime.New(logger, agents)
	sess := session.New(cwd, logger, session.WithUserMessage("", "Tell me a story about my current directory"))
	messages, err := rt.Run(ctx, sess)
	if err != nil {
		return err
	}

	fmt.Println(messages[len(messages)-1].Message.Content)
	return nil
}
