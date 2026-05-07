package runtime

import (
	"context"
	"log/slog"
	"time"

	"github.com/docker/docker-agent/pkg/hooks"
	"github.com/docker/docker-agent/pkg/model/provider"
)

// BuiltinUnload is the on_agent_switch builtin that asks every model on
// the previous agent to release its resources. Opt in via:
//
//	hooks:
//	  on_agent_switch:
//	    - type: builtin
//	      command: unload
//
// Today only Docker Model Runner ships a [provider.Unloader]; other
// providers are silently skipped, so wiring the builtin on a
// cross-provider chain is harmless.
const BuiltinUnload = "unload"

// unloadTimeout caps each Unload call so a stalled engine cannot stall
// agent switching.
const unloadTimeout = 10 * time.Second

// unloadBuiltin calls Unload on every [provider.Unloader] of the
// previous agent. Errors are logged but never propagated — agent
// switching must never block on a slow or unreachable engine.
func (r *LocalRuntime) unloadBuiltin(ctx context.Context, in *hooks.Input, _ []string) (*hooks.Output, error) {
	if in.FromAgent == "" || in.FromAgent == in.ToAgent {
		return nil, nil
	}
	from, err := r.team.Agent(in.FromAgent)
	if err != nil {
		slog.DebugContext(ctx, "unload: from-agent lookup failed",
			"agent", in.FromAgent, "error", err)
		return nil, nil
	}
	for _, p := range from.ConfiguredModels() {
		u, ok := p.(provider.Unloader)
		if !ok {
			continue
		}
		callCtx, cancel := context.WithTimeout(ctx, unloadTimeout)
		if err := u.Unload(callCtx); err != nil {
			slog.WarnContext(ctx, "unload: provider unload failed",
				"agent", from.Name(), "model", p.ID(), "error", err)
		}
		cancel()
	}
	return nil, nil
}
