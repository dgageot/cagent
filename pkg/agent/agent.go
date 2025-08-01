package agent

import (
	"context"
	"fmt"
	"math/rand"
	"sync/atomic"

	"github.com/docker/cagent/pkg/memorymanager"
	"github.com/docker/cagent/pkg/model/provider"
	"github.com/docker/cagent/pkg/tools"
)

// Agent represents an AI agent
type Agent struct {
	name               string
	description        string
	instruction        string
	toolsets           []tools.ToolSet
	toolsetsStarted    atomic.Bool
	models             []provider.Provider
	subAgents          []*Agent
	parents            []*Agent
	toolwrapper        toolwrapper
	memoryManager      memorymanager.Manager
	addPromptFile      string
	addDate            bool
	addEnvironmentInfo bool
}

// New creates a new agent
func New(name, prompt string, opts ...Opt) *Agent {
	agent := &Agent{
		name:        name,
		instruction: prompt,
	}

	for _, opt := range opts {
		opt(agent)
	}

	return agent
}

func (a *Agent) Name() string {
	return a.name
}

// Instruction returns the agent's instructions
func (a *Agent) Instruction() string {
	return a.instruction
}

func (a *Agent) AddDate() bool {
	return a.addDate
}

func (a *Agent) AddEnvironmentInfo() bool {
	return a.addEnvironmentInfo
}

func (a *Agent) AddPromptFile() string {
	return a.addPromptFile
}

// Description returns the agent's description
func (a *Agent) Description() string {
	return a.description
}

// SubAgents returns the list of sub-agent names
func (a *Agent) SubAgents() []*Agent {
	return a.subAgents
}

// Parents returns the list of parent agent names
func (a *Agent) Parents() []*Agent {
	return a.parents
}

// HasSubAgents checks if the agent has sub-agents
func (a *Agent) HasSubAgents() bool {
	return len(a.subAgents) > 0
}

func (a *Agent) HasParents() bool {
	return len(a.parents) > 0
}

// Model returns a random model from the available models
func (a *Agent) Model() provider.Provider {
	return a.models[rand.Intn(len(a.models))]
}

// Tools returns the tools available to this agent
func (a *Agent) Tools(ctx context.Context) ([]tools.Tool, error) {
	if err := a.ensureToolSetsAreStarted(ctx); err != nil {
		return nil, err
	}

	agentTools := []tools.Tool{}
	for _, toolSet := range a.toolsets {
		ta, err := toolSet.Tools(ctx)
		if err != nil {
			return nil, fmt.Errorf("failed to get tools: %w", err)
		}
		agentTools = append(agentTools, ta...)
	}

	agentTools = append(agentTools, a.toolwrapper.allTools...)

	return agentTools, nil
}

func (a *Agent) ToolSets() []tools.ToolSet {
	return a.toolsets
}

func (a *Agent) ensureToolSetsAreStarted(ctx context.Context) error {
	if a.toolsetsStarted.Load() {
		return nil
	}

	for _, toolSet := range a.toolsets {
		if err := toolSet.Start(ctx); err != nil {
			return fmt.Errorf("failed to start toolset: %w", err)
		}
	}

	a.toolsetsStarted.Store(true)
	return nil
}

func (a *Agent) StopToolSets() error {
	if !a.toolsetsStarted.Load() {
		return nil
	}

	for _, toolSet := range a.toolsets {
		if err := toolSet.Stop(); err != nil {
			return fmt.Errorf("failed to stop toolset: %w", err)
		}
	}

	a.toolsetsStarted.Store(false)
	return nil
}
