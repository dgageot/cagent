package runtime

import (
	"context"
	"errors"
	"sync/atomic"
	"testing"

	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"

	"github.com/docker/docker-agent/pkg/agent"
	"github.com/docker/docker-agent/pkg/chat"
	"github.com/docker/docker-agent/pkg/config/latest"
	"github.com/docker/docker-agent/pkg/model/provider/base"
	"github.com/docker/docker-agent/pkg/team"
	"github.com/docker/docker-agent/pkg/tools"
)

// unloadingProvider is a [provider.Unloader] that lets tests count and
// inspect the calls the runtime makes when switching agents.
type unloadingProvider struct {
	id          string
	stream      chat.MessageStream
	cfg         latest.ModelConfig
	calls       atomic.Int32
	returnError error
}

func (m *unloadingProvider) ID() string { return m.id }

func (m *unloadingProvider) CreateChatCompletionStream(context.Context, []chat.Message, []tools.Tool) (chat.MessageStream, error) {
	if m.stream != nil {
		return m.stream, nil
	}
	return &mockStream{}, nil
}

func (m *unloadingProvider) BaseConfig() base.Config { return base.Config{ModelConfig: m.cfg} }

func (m *unloadingProvider) MaxTokens() int { return 0 }

func (m *unloadingProvider) Unload(context.Context) error {
	m.calls.Add(1)
	return m.returnError
}

func newUnloadingAgentRuntime(t *testing.T, providers map[string]*unloadingProvider) *LocalRuntime {
	t.Helper()
	var agents []*agent.Agent
	for name, p := range providers {
		agents = append(agents, agent.New(name, name+" instructions", agent.WithModel(p)))
	}
	tm := team.New(team.WithAgents(agents...))
	rt, err := NewLocalRuntime(tm, WithModelStore(mockModelStore{}))
	require.NoError(t, err)
	return rt
}

func TestUnloadOnSwitch_OptedInModelIsUnloaded(t *testing.T) {
	t.Parallel()

	prev := &unloadingProvider{
		id: "dmr/qwen3",
		cfg: latest.ModelConfig{
			Provider: "dmr",
			Model:    "ai/qwen3",
			ProviderOpts: map[string]any{
				"unload_on_switch": true,
			},
		},
	}
	next := &unloadingProvider{
		id: "dmr/llama3.2",
		cfg: latest.ModelConfig{
			Provider: "dmr",
			Model:    "ai/llama3.2",
		},
	}

	rt := newUnloadingAgentRuntime(t, map[string]*unloadingProvider{
		"prev": prev,
		"next": next,
	})

	prevAgent, err := rt.team.Agent("prev")
	require.NoError(t, err)
	nextAgent, err := rt.team.Agent("next")
	require.NoError(t, err)

	rt.unloadOnSwitch(t.Context(), prevAgent, nextAgent)

	assert.Equal(t, int32(1), prev.calls.Load(), "previous agent's model must be unloaded")
	assert.Equal(t, int32(0), next.calls.Load(), "next agent's model must NOT be unloaded")
}

func TestUnloadOnSwitch_NoOptInDoesNothing(t *testing.T) {
	t.Parallel()

	prev := &unloadingProvider{
		id: "dmr/qwen3",
		cfg: latest.ModelConfig{
			Provider: "dmr",
			Model:    "ai/qwen3",
			// No unload_on_switch.
		},
	}
	next := &unloadingProvider{
		id:  "dmr/llama3.2",
		cfg: latest.ModelConfig{Provider: "dmr", Model: "ai/llama3.2"},
	}

	rt := newUnloadingAgentRuntime(t, map[string]*unloadingProvider{
		"prev": prev,
		"next": next,
	})

	prevAgent, err := rt.team.Agent("prev")
	require.NoError(t, err)
	nextAgent, err := rt.team.Agent("next")
	require.NoError(t, err)

	rt.unloadOnSwitch(t.Context(), prevAgent, nextAgent)
	assert.Equal(t, int32(0), prev.calls.Load(),
		"unload must NOT happen when the model didn't opt in")
}

func TestUnloadOnSwitch_SameAgentIsNoOp(t *testing.T) {
	t.Parallel()

	prev := &unloadingProvider{
		id: "dmr/qwen3",
		cfg: latest.ModelConfig{
			ProviderOpts: map[string]any{"unload_on_switch": true},
		},
	}

	rt := newUnloadingAgentRuntime(t, map[string]*unloadingProvider{
		"only": prev,
	})

	a, err := rt.team.Agent("only")
	require.NoError(t, err)
	rt.unloadOnSwitch(t.Context(), a, a)
	assert.Equal(t, int32(0), prev.calls.Load(),
		"unload must NOT happen when prev == next")
}

func TestUnloadOnSwitch_NilPrevIsNoOp(t *testing.T) {
	t.Parallel()

	next := &unloadingProvider{
		id:  "dmr/qwen3",
		cfg: latest.ModelConfig{},
	}

	rt := newUnloadingAgentRuntime(t, map[string]*unloadingProvider{"next": next})
	a, err := rt.team.Agent("next")
	require.NoError(t, err)

	rt.unloadOnSwitch(t.Context(), nil, a)
	assert.Equal(t, int32(0), next.calls.Load())
}

func TestUnloadOnSwitch_ProviderWithoutUnloaderIsSkipped(t *testing.T) {
	t.Parallel()

	// mockProvider is the runtime test fixture without an Unload method.
	prev := &mockProvider{id: "openai/gpt-4"}
	next := &mockProvider{id: "openai/gpt-3.5"}

	prevAgent := agent.New("prev", "prev", agent.WithModel(prev))
	nextAgent := agent.New("next", "next", agent.WithModel(next))
	tm := team.New(team.WithAgents(prevAgent, nextAgent))
	rt, err := NewLocalRuntime(tm, WithModelStore(mockModelStore{}))
	require.NoError(t, err)

	// Should not panic, even though the provider doesn't implement Unloader.
	rt.unloadOnSwitch(t.Context(), prevAgent, nextAgent)
}

func TestUnloadOnSwitch_UnloadErrorDoesNotPropagate(t *testing.T) {
	t.Parallel()

	prev := &unloadingProvider{
		id: "dmr/qwen3",
		cfg: latest.ModelConfig{
			ProviderOpts: map[string]any{"unload_on_switch": true},
		},
		returnError: errors.New("engine unreachable"),
	}
	next := &unloadingProvider{id: "dmr/llama3.2"}

	rt := newUnloadingAgentRuntime(t, map[string]*unloadingProvider{
		"prev": prev,
		"next": next,
	})

	prevAgent, err := rt.team.Agent("prev")
	require.NoError(t, err)
	nextAgent, err := rt.team.Agent("next")
	require.NoError(t, err)

	// Must not panic and must not return an error.
	rt.unloadOnSwitch(t.Context(), prevAgent, nextAgent)
	assert.Equal(t, int32(1), prev.calls.Load(),
		"Unload should still be invoked even though it returns an error")
}
