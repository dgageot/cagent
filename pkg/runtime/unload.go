package runtime

import (
	"context"
	"log/slog"
	"time"

	"github.com/docker/docker-agent/pkg/agent"
	"github.com/docker/docker-agent/pkg/model/provider"
)

// unloadOnSwitchTimeout caps each Unload() call. The runtime invokes
// Unload during agent switches; a stalled or unreachable engine must not
// block the user, so we time out aggressively and keep going.
const unloadOnSwitchTimeout = 10 * time.Second

// unloadOnSwitch asks the previous agent's models to release any
// resources they hold, when they have explicitly opted in via
// `provider_opts.unload_on_switch: true`. It is best-effort: providers
// that don't implement [provider.Unloader] are silently skipped, and any
// error from Unload is logged but never propagated, so a slow or
// unreachable engine cannot break agent switching.
//
// Every configured model is considered (not just the currently-selected
// one) so alloyed agents free every variant they may have loaded.
func (r *LocalRuntime) unloadOnSwitch(ctx context.Context, prev, next *agent.Agent) {
	if prev == nil || prev == next {
		return
	}
	for _, m := range prev.ConfiguredModels() {
		if m == nil {
			continue
		}
		cfg := m.BaseConfig().ModelConfig
		if !cfg.UnloadOnSwitch() {
			continue
		}
		unloader, ok := m.(provider.Unloader)
		if !ok {
			continue
		}
		callCtx, cancel := context.WithTimeout(ctx, unloadOnSwitchTimeout)
		if err := unloader.Unload(callCtx); err != nil {
			slog.Warn("unload-on-switch failed",
				"agent", prev.Name(), "model", m.ID(), "error", err)
		}
		cancel()
	}
}
