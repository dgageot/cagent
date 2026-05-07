package runtime

import (
	"context"
	"os"

	"github.com/docker/docker-agent/pkg/hooks/builtins"
	"github.com/docker/docker-agent/pkg/session"
)

// WithSnapshots configures whether snapshot hooks are auto-injected for every agent.
func WithSnapshots(enabled bool) Opt {
	return func(r *LocalRuntime) {
		r.snapshotsEnabled = enabled
	}
}

// SnapshotsEnabled reports whether automatic snapshot hooks are active for
// this runtime. Used by the TUI to hide snapshot-related commands when the
// feature is off.
func (r *LocalRuntime) SnapshotsEnabled() bool {
	return r != nil && r.snapshotsEnabled
}

// UndoLastSnapshot restores files recorded for the latest completed snapshot hook checkpoint.
func (r *LocalRuntime) UndoLastSnapshot(ctx context.Context, sess *session.Session) (int, bool, error) {
	cwd := r.snapshotCwd(sess)
	if cwd == "" {
		return 0, false, nil
	}
	return r.builtinsState.UndoLastSnapshot(ctx, sess.ID, cwd)
}

// ListSnapshots returns the completed snapshot checkpoints recorded for the
// session, oldest first. Returns nil when none exist.
func (r *LocalRuntime) ListSnapshots(sess *session.Session) []builtins.SnapshotInfo {
	if r == nil || sess == nil {
		return nil
	}
	return r.builtinsState.ListSnapshots(sess.ID)
}

// ResetSnapshot reverts every checkpoint past index keep so the workspace
// returns to the state captured at that snapshot. keep == 0 resets to the
// original (pre-agent) state.
func (r *LocalRuntime) ResetSnapshot(ctx context.Context, sess *session.Session, keep int) (int, bool, error) {
	cwd := r.snapshotCwd(sess)
	if cwd == "" {
		return 0, false, nil
	}
	return r.builtinsState.ResetSnapshot(ctx, sess.ID, cwd, keep)
}

// snapshotCwd resolves the working directory used to open the shadow
// repository for snapshot operations. Returns "" when no candidate is usable.
func (r *LocalRuntime) snapshotCwd(sess *session.Session) string {
	if r == nil || sess == nil {
		return ""
	}
	if sess.WorkingDir != "" {
		return sess.WorkingDir
	}
	if r.workingDir != "" {
		return r.workingDir
	}
	cwd, _ := os.Getwd()
	return cwd
}
