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
		logger)
	if err != nil {
		return err
	}

	child := agent.New(
		"child",
		"You are a child, with a lot of energy.",
		agent.WithModel(llm),
		agent.WithDescription("A child."),
	)
	root := agent.New(
		"root",
		"You are a human, with feelings and emotions.",
		agent.WithModel(llm),
		agent.WithSubAgents(child),
		agent.WithToolSets(builtin.NewTransferTaskTool()),
	)

	rt := runtime.New(logger, team.New(team.WithAgents(root, child)))
	sess := session.New(cwd, logger, session.WithUserMessage("", "Ask your child how they are doing and tell me what they said"))
	messages, err := rt.Run(ctx, sess)
	if err != nil {
		return err
	}

	fmt.Println(messages[len(messages)-1].Message.Content)
	return nil
}
