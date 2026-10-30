package agent

import (
	"context"
	"fmt"
	"slices"
)

// ForWorkingDir builds an independent agent for dir, reusing provider clients
// and snapshotting runtime model and harness-effort overrides.
func (a *Agent) ForWorkingDir(ctx context.Context, dir string) (*Agent, error) {
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	if a.workingDirFactory == nil {
		return nil, fmt.Errorf("agent %q has no working directory factory", a.name)
	}
	rebuilt, err := a.workingDirFactory(ctx, dir)
	if err != nil {
		return nil, fmt.Errorf("agent %q: creating workspace: %w", a.name, err)
	}
	if rebuilt == nil || rebuilt == a {
		return nil, fmt.Errorf("agent %q: working directory factory must return a new agent", a.name)
	}
	rebuilt.name = a.name
	rebuilt.workingDirFactory = a.workingDirFactory
	if overrides := a.modelOverrides.Load(); overrides != nil {
		models := slices.Clone(*overrides)
		rebuilt.modelOverrides.Store(&models)
	}
	if effort := a.harnessEffort.Load(); effort != nil {
		rebuilt.SetHarnessEffort(*effort)
	}
	return rebuilt, nil
}
