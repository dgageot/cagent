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
	"github.com/docker/docker-agent/pkg/hooks"
	"github.com/docker/docker-agent/pkg/model/provider"
	"github.com/docker/docker-agent/pkg/model/provider/base"
	"github.com/docker/docker-agent/pkg/team"
	"github.com/docker/docker-agent/pkg/tools"
)

// unloadingProvider is a minimal [provider.Unloader] test double that
// counts Unload calls and (optionally) returns a configured error so
// tests can assert call sites and best-effort error swallowing.
type unloadingProvider struct {
	id        string
	calls     atomic.Int64
	unloadErr error
}

func (p *unloadingProvider) ID() string { return p.id }

func (p *unloadingProvider) CreateChatCompletionStream(context.Context, []chat.Message, []tools.Tool) (chat.MessageStream, error) {
	return &mockStream{}, nil
}

func (p *unloadingProvider) BaseConfig() base.Config {
	return base.Config{ModelConfig: latest.ModelConfig{Provider: "test", Model: p.id}}
}

func (p *unloadingProvider) Unload(_ context.Context) error {
	p.calls.Add(1)
	return p.unloadErr
}

var _ provider.Unloader = (*unloadingProvider)(nil)

// newUnloadingRuntime wires up agents named after the supplied provider
// IDs and returns a runtime ready to drive [unloadBuiltin]. Each name
// gets its own [unloadingProvider] reachable through the returned map.
func newUnloadingRuntime(t *testing.T, names ...string) (*LocalRuntime, map[string]*unloadingProvider) {
	t.Helper()
	provs := make(map[string]*unloadingProvider, len(names))
	agents := make([]*agent.Agent, len(names))
	for i, name := range names {
		p := &unloadingProvider{id: name}
		provs[name] = p
		agents[i] = agent.New(name, "instructions", agent.WithModel(p))
	}
	rt, err := NewLocalRuntime(team.New(team.WithAgents(agents...)),
		WithSessionCompaction(false), WithModelStore(mockModelStore{}))
	require.NoError(t, err)
	return rt, provs
}

func TestUnloadBuiltin(t *testing.T) {
	t.Parallel()

	t.Run("calls Unload on the previous agent only", func(t *testing.T) {
		t.Parallel()
		rt, provs := newUnloadingRuntime(t, "from", "to")
		_, err := rt.unloadBuiltin(t.Context(), &hooks.Input{FromAgent: "from", ToAgent: "to"}, nil)
		require.NoError(t, err)
		assert.Equal(t, int64(1), provs["from"].calls.Load(), "from-agent's model must be unloaded once")
		assert.Equal(t, int64(0), provs["to"].calls.Load(), "to-agent's model must NOT be unloaded")
	})

	t.Run("swallows Unload errors so agent switch never blocks", func(t *testing.T) {
		t.Parallel()
		rt, provs := newUnloadingRuntime(t, "from", "to")
		provs["from"].unloadErr = errors.New("engine offline")
		out, err := rt.unloadBuiltin(t.Context(), &hooks.Input{FromAgent: "from", ToAgent: "to"}, nil)
		require.NoError(t, err)
		assert.Nil(t, out)
		assert.Equal(t, int64(1), provs["from"].calls.Load())
	})

	t.Run("no-op when from==to", func(t *testing.T) {
		t.Parallel()
		rt, provs := newUnloadingRuntime(t, "from")
		_, err := rt.unloadBuiltin(t.Context(), &hooks.Input{FromAgent: "from", ToAgent: "from"}, nil)
		require.NoError(t, err)
		assert.Equal(t, int64(0), provs["from"].calls.Load())
	})
}

// TestUnloadBuiltin_Registered guarantees the builtin name is findable
// on the runtime registry so YAML hook entries that reference it
// actually resolve.
func TestUnloadBuiltin_Registered(t *testing.T) {
	t.Parallel()
	rt, _ := newUnloadingRuntime(t, "loader")
	fn, ok := rt.hooksRegistry.LookupBuiltin(BuiltinUnload)
	require.True(t, ok, "%q must be registered on the runtime's hook registry", BuiltinUnload)
	require.NotNil(t, fn)
}
