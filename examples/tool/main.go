package main

import (
	"context"
	"encoding/json"
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
	"github.com/docker/cagent/pkg/tools"
)

func addNumbers(ctx context.Context, toolCall tools.ToolCall) (*tools.ToolCallResult, error) {
	type params struct {
		A int `json:"a"`
		B int `json:"b"`
	}

	var p params
	if err := json.Unmarshal([]byte(toolCall.Function.Arguments), &p); err != nil {
		return nil, err
	}

	fmt.Println("Adding numbers", p.A, p.B)

	return &tools.ToolCallResult{
		Output: fmt.Sprintf("%d", p.A+p.B),
	}, nil
}

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

	toolAddNumbers := tools.Tool{
		Handler: addNumbers,
		Function: &tools.FunctionDefinition{
			Name:        "add",
			Description: "Add two numbers",
			Parameters: tools.FunctionParamaters{
				Type: "object",
				Properties: map[string]any{
					"a": map[string]any{
						"type": "number",
					},
					"b": map[string]any{
						"type": "number",
					},
				},
			},
		},
	}

	calculator := agent.New(
		"root",
		"You are a human, with feelings and emotions.",
		agent.WithModel(llm),
		agent.WithTools(toolAddNumbers),
	)
	calculatorTeam := team.New(team.WithAgents(calculator))

	rt := runtime.New(logger, calculatorTeam)
	sess := session.New(cwd, logger, session.WithUserMessage("", "What is 1 + 2?"))
	messages, err := rt.Run(ctx, sess)
	if err != nil {
		return err
	}

	fmt.Println(messages[len(messages)-1].Message.Content)
	return nil
}
