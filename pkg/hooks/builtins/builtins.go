// Package builtins contains the stock in-process hook implementations
// shipped with docker-agent.
//
// Available builtins:
//
//   - add_date              (turn_start)      — today's date
//   - add_environment_info  (session_start)   — cwd, git, OS, arch
//   - add_prompt_files      (turn_start)      — contents of prompt files
//   - add_git_status        (turn_start)      — `git status --short --branch`
//   - add_git_diff          (turn_start)      — `git diff --stat` (or full)
//   - add_directory_listing (session_start)   — top-level entries of cwd
//   - add_user_info         (session_start)   — current OS user and host
//   - add_recent_commits    (session_start)   — `git log --oneline -n N`
//   - max_iterations        (before_llm_call) — hard stop after N model calls
//   - snapshot              (session_start,
//     turn_start, turn_end,
//     pre_tool_use, post_tool_use,
//     session_end) — shadow-git snapshots
//   - redact_secrets        (pre_tool_use,
//     before_llm_call,
//     tool_response_transform) — scrub secrets
//     from tool args, outgoing chat content, and
//     tool output. Same builtin, dispatches on
//     event so a single name covers all three
//     legs of the feature.
//
// Reference any of them from a hook YAML entry as
// `{type: builtin, command: "<name>"}`. The runtime additionally
// auto-injects add_date / add_environment_info / add_prompt_files /
// redact_secrets from the matching agent flags, and snapshot from
// global user config. Setting redact_secrets at the agent level is
// exactly equivalent to writing
// the three matching hook entries by hand —
// [ApplyAgentDefaults] performs the auto-injection.
//
// turn_start builtins recompute every turn (date, git state).
// session_start builtins run once per session for context that's
// stable for its duration. max_iterations is stateful: its
// per-session counter lives on the [State] returned by [Register];
// the runtime clears it via [State.ClearSession] from session_end.
// snapshot is also stateful: it keeps per-session turn/tool snapshot
// hashes and undo checkpoints in memory while the shadow git objects live
// under the data directory. Undo checkpoints intentionally survive the
// RunStream session_end cleanup so /undo can run after the response stops.
//
// LLM-as-a-judge hooks are NOT shipped here: write `type: model` with
// `schema: pre_tool_use_decision` instead — see
// pkg/hooks/shape_pre_tool_use_decision.go and examples/llm_judge.yaml.
package builtins

import (
	"context"
	"errors"

	"github.com/docker/docker-agent/pkg/hooks"
)

// State holds the per-runtime state of the stateful builtins.
// It is returned by [Register] so callers can clear per-session
// entries on session_end. Stateless builtins don't appear here.
type State struct {
	maxIterations *maxIterationsBuiltin
	snapshot      *snapshotBuiltin
}

// ClearSession drops per-RunStream state that should not survive stream teardown.
// A nil receiver is a no-op.
func (s *State) ClearSession(sessionID string) {
	if s == nil || sessionID == "" {
		return
	}
	s.maxIterations.clearSession(sessionID)
}

// UndoLastSnapshot restores files from the latest completed snapshot checkpoint.
func (s *State) UndoLastSnapshot(ctx context.Context, sessionID, cwd string) (files int, ok bool, err error) {
	if s == nil || s.snapshot == nil || sessionID == "" || cwd == "" {
		return 0, false, nil
	}
	return s.snapshot.undoLast(ctx, sessionID, cwd)
}

// ListSnapshots returns the completed snapshot checkpoints for a session in
// chronological order (oldest first). Returns nil when no snapshots exist.
func (s *State) ListSnapshots(sessionID string) []SnapshotInfo {
	if s == nil || s.snapshot == nil || sessionID == "" {
		return nil
	}
	return s.snapshot.listSnapshots(sessionID)
}

// ResetSnapshot reverts every checkpoint past index keep so the workspace
// returns to the state captured at that snapshot. keep == 0 resets to the
// original (pre-agent) state.
func (s *State) ResetSnapshot(ctx context.Context, sessionID, cwd string, keep int) (files int, ok bool, err error) {
	if s == nil || s.snapshot == nil || sessionID == "" || cwd == "" {
		return 0, false, nil
	}
	return s.snapshot.resetSnapshot(ctx, sessionID, cwd, keep)
}

// Register installs the stock builtin hooks on r and returns a [State]
// handle the caller can use for stateful builtin operations.
func Register(r *hooks.Registry) (*State, error) {
	state := &State{
		maxIterations: newMaxIterations(),
		snapshot:      newSnapshotBuiltin(),
	}
	if err := errors.Join(
		r.RegisterBuiltin(AddDate, addDate),
		r.RegisterBuiltin(AddEnvironmentInfo, addEnvironmentInfo),
		r.RegisterBuiltin(AddPromptFiles, addPromptFiles),
		r.RegisterBuiltin(AddGitStatus, addGitStatus),
		r.RegisterBuiltin(AddGitDiff, addGitDiff),
		r.RegisterBuiltin(AddDirectoryListing, addDirectoryListing),
		r.RegisterBuiltin(AddUserInfo, addUserInfo),
		r.RegisterBuiltin(AddRecentCommits, addRecentCommits),
		r.RegisterBuiltin(MaxIterations, state.maxIterations.hook),
		r.RegisterBuiltin(Snapshot, state.snapshot.hook),
		r.RegisterBuiltin(RedactSecrets, redactSecrets),
	); err != nil {
		return nil, err
	}
	return state, nil
}

// AgentDefaults captures defaults that map onto stock builtin hook entries.
// Pass each AgentConfig.AddXxx flag as-is; Snapshot comes from runtime/global config.
type AgentDefaults struct {
	AddDate            bool
	AddEnvironmentInfo bool
	AddPromptFiles     []string
	// RedactSecrets auto-injects the redact_secrets builtin under
	// pre_tool_use, before_llm_call, and tool_response_transform — the
	// three legs of the feature. Equivalent to writing those three
	// hook entries by hand; the dedup in [hooks.Executor.hooksFor]
	// makes the auto-injection idempotent against an explicit YAML
	// entry that already names the same builtin.
	RedactSecrets bool
	// Snapshot auto-injects shadow-git snapshots at
	// turn boundaries. session_end is included to garbage-collect the
	// shadow repository; undo history remains available after a response stops.
	Snapshot bool
}

// ApplyAgentDefaults appends the stock builtin hook entries implied by
// d to cfg. A nil cfg is treated as empty. Returns nil iff no hook
// (user-configured or auto-injected) is present.
func ApplyAgentDefaults(cfg *hooks.Config, d AgentDefaults) *hooks.Config {
	if cfg == nil {
		cfg = &hooks.Config{}
	}
	if d.AddDate {
		cfg.TurnStart = append(cfg.TurnStart, builtinHook(AddDate))
	}
	if len(d.AddPromptFiles) > 0 {
		cfg.TurnStart = append(cfg.TurnStart, builtinHook(AddPromptFiles, d.AddPromptFiles...))
	}
	if d.AddEnvironmentInfo {
		cfg.SessionStart = append(cfg.SessionStart, builtinHook(AddEnvironmentInfo))
	}
	if d.Snapshot {
		cfg.SessionStart = append(cfg.SessionStart, builtinHook(Snapshot))
		cfg.TurnStart = append(cfg.TurnStart, builtinHook(Snapshot))
		cfg.TurnEnd = append(cfg.TurnEnd, builtinHook(Snapshot))
		cfg.SessionEnd = append(cfg.SessionEnd, builtinHook(Snapshot))
	}
	if d.RedactSecrets {
		// Wire all three legs at once. The same builtin handles each
		// event — it dispatches on input.HookEventName — so a single
		// `command: redact_secrets` entry would already work, but we
		// inject explicit entries here so the resulting effective
		// config is self-describing (a user inspecting it sees that
		// args, messages, and tool output are all covered, without
		// having to read the dispatch table).
		cfg.PreToolUse = append(cfg.PreToolUse, hooks.MatcherConfig{
			Matcher: "*",
			Hooks:   []hooks.Hook{builtinHook(RedactSecrets)},
		})
		cfg.BeforeLLMCall = append(cfg.BeforeLLMCall, builtinHook(RedactSecrets))
		cfg.ToolResponseTransform = append(cfg.ToolResponseTransform, hooks.MatcherConfig{
			Matcher: "*",
			Hooks:   []hooks.Hook{builtinHook(RedactSecrets)},
		})
	}
	if cfg.IsEmpty() {
		return nil
	}
	return cfg
}

// builtinHook returns a hook entry that dispatches to the named builtin.
func builtinHook(name string, args ...string) hooks.Hook {
	return hooks.Hook{Type: hooks.HookTypeBuiltin, Command: name, Args: args}
}
