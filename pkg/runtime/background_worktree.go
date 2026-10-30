package runtime

import (
	"context"
	"fmt"
	"log/slog"
	"slices"
	"time"
	"uuid"

	"github.com/docker/docker-agent/pkg/agent"
	"github.com/docker/docker-agent/pkg/hooks"
	"github.com/docker/docker-agent/pkg/session"
	agenttool "github.com/docker/docker-agent/pkg/tools/builtin/agent"
	"github.com/docker/docker-agent/pkg/worktree"
)

func (r *LocalRuntime) prepareBackgroundWorktree(ctx context.Context, params agenttool.RunParams) (*worktree.Worktree, *LocalRuntime, error) {
	target, err := r.team.Agent(params.AgentName)
	if err != nil {
		return nil, nil, err
	}
	if target.HasHarness() {
		return nil, nil, fmt.Errorf("external harness agent %q does not support isolated background worktrees", params.AgentName)
	}
	var lookup session.Lookup
	if r.sessionStore != nil {
		lookup = r.sessionStore.GetSession
	}
	dir, err := session.ResolveWorkingDir(ctx, params.ParentSession, lookup)
	if err != nil {
		return nil, nil, fmt.Errorf("creating background worktree: %w", err)
	}
	wt, err := worktree.Create(ctx, dir, "agent-"+uuid.NewV4().String())
	if err != nil {
		return nil, nil, fmt.Errorf("creating background worktree: %w", err)
	}
	if params.OnWorktree != nil {
		params.OnWorktree(wt)
	}
	// Keep the checkout even on setup failure: hooks may already have written files.
	fail := func(err error) (*worktree.Worktree, *LocalRuntime, error) {
		return nil, nil, fmt.Errorf("background worktree retained at %s (branch %s): %w", wt.Dir, wt.Branch, err)
	}
	isolatedTeam, err := r.team.ForWorkingDir(ctx, wt.Dir)
	if err != nil {
		return fail(err)
	}
	for _, name := range isolatedTeam.AgentNames() {
		a, _ := isolatedTeam.Agent(name)
		// Response-cache keys do not include the delegated task or checkout.
		agent.WithCache(nil)(a)
	}
	r.ensureBudget()
	opts := []Opt{
		WithWorkingDir(wt.Dir), WithCurrentAgent(params.AgentName),
		WithSessionStore(r.sessionStore), WithModelStore(r.modelsStore), WithProviderRegistry(r.providerRegistry),
		WithEnv(r.env), WithClock(r.now), WithTelemetry(r.telemetry),
		WithNonInteractive(true), WithSteerQueue(NewInMemoryMessageQueue(defaultSteerQueueCapacity)),
		WithFollowUpQueue(NewInMemoryMessageQueue(defaultFollowUpQueueCapacity)),
		WithHooksRegistry(r.hooksRegistry.Clone()), withSharedBudget(r.currentBudget()),
		WithTracer(r.tracer), WithSessionCompaction(r.sessionCompaction),
		WithManagedOAuth(r.managedOAuth), WithUnmanagedOAuthRedirectURI(r.unmanagedOAuthRedirectURI),
		WithToolListTimeout(r.toolListTimeout), WithToolStartTimeout(r.toolStartTimeout),
		WithMaxOverflowCompactions(r.maxOverflowCompactions), WithModelSwitcherConfig(r.modelSwitcherCfg),
	}
	for _, injector := range r.autoInjectors {
		opts = append(opts, WithAutoInjector(injector))
	}
	if r.fallback.retryOnRateLimit {
		opts = append(opts, WithRetryOnRateLimit())
	}
	if r.commandEvaluator != nil {
		opts = append(opts, WithCommandEvaluatorFactory(*r.commandEvaluator))
	}
	if r.harnessFactory != nil {
		opts = append(opts, WithHarnessFactory(*r.harnessFactory))
	}
	for _, transform := range r.transforms {
		if transform.name != BuiltinStripGeneratedMedia && transform.name != BuiltinStripUnsupportedModalities {
			opts = append(opts, WithMessageTransform(transform.name, transform.fn))
		}
	}
	isolated, err := NewLocalRuntime(ctx, isolatedTeam, opts...)
	if err != nil {
		stopCtx, cancel := context.WithTimeout(context.WithoutCancel(ctx), 30*time.Second)
		defer cancel()
		if stopErr := isolatedTeam.StopToolSets(stopCtx); stopErr != nil {
			slog.WarnContext(ctx, "Failed to stop isolated toolsets", "error", stopErr)
		}
		return fail(err)
	}
	isolated.observers = slices.Clone(r.observers)
	isolated.isolatedWorktree = true
	isolated.liveSessionParent = r
	isolated.OnBackgroundEventWithContext(r.emitBackgroundEvent)
	child, _ := isolatedTeam.Agent(params.AgentName)
	exec := isolated.hooksExec(child)
	if exec != nil && exec.Has(hooks.EventWorktreeCreate) {
		result, err := exec.Dispatch(ctx, hooks.EventWorktreeCreate, &hooks.Input{
			AgentName: params.AgentName, Cwd: wt.Dir, WorktreePath: wt.Dir,
			WorktreeBranch: wt.Branch, WorktreeSourceDir: wt.SourceDir,
		})
		if err == nil && !result.Allowed {
			err = fmt.Errorf("worktree_create hook blocked the task: %s", result.Message)
		}
		if err != nil {
			closeWorktreeRuntime(ctx, isolated)
			return fail(err)
		}
		if params.OnContent != nil {
			params.OnContent(result.SystemMessage + result.AdditionalContext)
		}
	}
	return wt, isolated, nil
}

func closeWorktreeRuntime(ctx context.Context, rt *LocalRuntime) {
	if err := rt.Close(); err != nil {
		slog.WarnContext(ctx, "Failed to close worktree runtime", "error", err)
	}
	stopCtx, cancel := context.WithTimeout(context.WithoutCancel(ctx), 30*time.Second)
	defer cancel()
	if err := rt.team.StopToolSets(stopCtx); err != nil {
		slog.WarnContext(ctx, "Failed to stop worktree toolsets", "error", err)
	}
}
