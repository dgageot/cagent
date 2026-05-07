package app

import (
	"context"
	"errors"
	"fmt"

	"github.com/docker/docker-agent/pkg/hooks/builtins"
	"github.com/docker/docker-agent/pkg/session"
)

var ErrNothingToUndo = errors.New("nothing to undo")

type UndoSnapshotResult struct {
	RestoredFiles int
}

// snapshotRuntime is the subset of the runtime API that the App needs to
// drive snapshot commands. Runtimes that don't capture snapshots (e.g.
// remote runtimes) simply don't implement this interface and the related
// commands are then disabled in the UI.
type snapshotRuntime interface {
	SnapshotsEnabled() bool
	UndoLastSnapshot(ctx context.Context, sess *session.Session) (files int, ok bool, err error)
	ListSnapshots(sess *session.Session) []builtins.SnapshotInfo
	ResetSnapshot(ctx context.Context, sess *session.Session, keep int) (files int, ok bool, err error)
}

// snapshotRuntime returns the runtime's snapshot interface, or nil when the
// runtime doesn't support snapshots at all (e.g. remote runtimes).
func (a *App) snapshotRuntime() snapshotRuntime {
	r, _ := a.runtime.(snapshotRuntime)
	return r
}

// SnapshotsEnabled reports whether automatic shadow-git snapshots are active
// for the current runtime. The answer doesn't depend on having an active
// session: it's a runtime/configuration capability check.
func (a *App) SnapshotsEnabled() bool {
	r := a.snapshotRuntime()
	return r != nil && r.SnapshotsEnabled()
}

// UndoLastSnapshot restores the files captured in the most recent snapshot.
func (a *App) UndoLastSnapshot(ctx context.Context) (UndoSnapshotResult, error) {
	r := a.snapshotRuntime()
	if r == nil || a.session == nil {
		return UndoSnapshotResult{}, ErrNothingToUndo
	}
	return snapshotResult(r.UndoLastSnapshot(ctx, a.session))
}

// ListSnapshots returns the file count of every snapshot captured during the
// current session, oldest first. Returns nil when no snapshots exist or when
// the runtime doesn't support them.
func (a *App) ListSnapshots() []int {
	r := a.snapshotRuntime()
	if r == nil || a.session == nil {
		return nil
	}
	infos := r.ListSnapshots(a.session)
	counts := make([]int, len(infos))
	for i, info := range infos {
		counts[i] = info.Files
	}
	return counts
}

// ResetSnapshot reverts every checkpoint past index keep so the workspace
// returns to the state captured at that snapshot. keep == 0 resets to the
// original pre-agent state.
func (a *App) ResetSnapshot(ctx context.Context, keep int) (UndoSnapshotResult, error) {
	r := a.snapshotRuntime()
	if r == nil || a.session == nil {
		return UndoSnapshotResult{}, ErrNothingToUndo
	}
	return snapshotResult(r.ResetSnapshot(ctx, a.session, keep))
}

// snapshotResult adapts the (files, ok, err) tuple returned by snapshot
// operations into the UndoSnapshotResult / ErrNothingToUndo shape callers
// expect.
func snapshotResult(files int, ok bool, err error) (UndoSnapshotResult, error) {
	if err != nil {
		return UndoSnapshotResult{}, fmt.Errorf("restoring snapshot: %w", err)
	}
	if !ok {
		return UndoSnapshotResult{}, ErrNothingToUndo
	}
	return UndoSnapshotResult{RestoredFiles: files}, nil
}
