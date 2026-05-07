package builtins

import (
	"context"
	"errors"
	"log/slog"
	"sync"

	"github.com/docker/docker-agent/pkg/hooks"
	"github.com/docker/docker-agent/pkg/snapshot"
)

// Snapshot is the registered name of the snapshot builtin.
const Snapshot = "snapshot"

// SnapshotInfo summarises one completed snapshot checkpoint for display.
type SnapshotInfo struct {
	// Files is the number of unique files captured in the checkpoint.
	Files int
}

type snapshotBuiltin struct {
	manager *snapshot.Manager
	mu      sync.Mutex
	session map[string]*snapshotSession
}

type snapshotSession struct {
	turn    string
	tools   map[string]string
	history []snapshotCheckpoint
}

type snapshotCheckpoint struct {
	hash  string
	files []string
}

func newSnapshotBuiltin() *snapshotBuiltin {
	return &snapshotBuiltin{
		manager: snapshot.NewManager(""),
		session: map[string]*snapshotSession{},
	}
}

func (b *snapshotBuiltin) hook(ctx context.Context, in *hooks.Input, _ []string) (*hooks.Output, error) {
	if in == nil || in.Cwd == "" || in.SessionID == "" {
		return nil, nil
	}
	repo, err := b.manager.Open(ctx, in.Cwd)
	if err != nil {
		if errors.Is(err, snapshot.ErrNotGitRepository) {
			return nil, nil
		}
		slog.DebugContext(ctx, "snapshot hook: open repository failed; skipping", "cwd", in.Cwd, "error", err)
		return nil, nil
	}

	track := func() (string, bool) {
		hash, err := repo.Track(ctx)
		if err != nil {
			slog.DebugContext(ctx, "snapshot hook: track failed", "cwd", in.Cwd, "error", err)
			return "", false
		}
		return hash, true
	}
	patchFrom := func(hash string) (snapshot.Patch, string, bool) {
		after, ok := track()
		if !ok {
			return snapshot.Patch{}, "", false
		}
		patch, err := repo.Patch(ctx, hash)
		if err != nil {
			slog.DebugContext(ctx, "snapshot hook: patch failed", "cwd", in.Cwd, "hash", hash, "error", err)
			return snapshot.Patch{}, "", false
		}
		return patch, after, true
	}

	switch in.HookEventName {
	case hooks.EventSessionStart:
		track()
	case hooks.EventTurnStart:
		if hash, ok := track(); ok {
			b.setTurn(in.SessionID, hash)
		}
	case hooks.EventPreToolUse:
		if hash, ok := track(); ok {
			b.setTool(in.SessionID, in.ToolUseID, hash)
		}
	case hooks.EventPostToolUse:
		hash := b.popTool(in.SessionID, in.ToolUseID)
		if hash == "" {
			return nil, nil
		}
		if patch, after, ok := patchFrom(hash); ok {
			b.pushCheckpoint(in.SessionID, snapshotCheckpoint{hash: hash, files: patch.Files})
			logPatch(ctx, "tool", in.SessionID, in.ToolName, patch, after)
		}
	case hooks.EventTurnEnd:
		hash := b.popTurn(in.SessionID)
		if hash == "" {
			return nil, nil
		}
		if patch, after, ok := patchFrom(hash); ok {
			b.pushCheckpoint(in.SessionID, snapshotCheckpoint{hash: hash, files: patch.Files})
			logPatch(ctx, "turn", in.SessionID, in.Reason, patch, after)
		}
	case hooks.EventSessionEnd:
		if err := repo.Cleanup(ctx); err != nil {
			slog.DebugContext(ctx, "snapshot hook: cleanup failed", "cwd", in.Cwd, "error", err)
		}
	default:
		slog.DebugContext(ctx, "snapshot hook configured on unsupported event; skipping", "event", in.HookEventName)
	}
	return nil, nil
}

func (b *snapshotBuiltin) getSession(sessionID string) *snapshotSession {
	s := b.session[sessionID]
	if s == nil {
		s = &snapshotSession{tools: map[string]string{}}
		b.session[sessionID] = s
	}
	return s
}

func (b *snapshotBuiltin) setTurn(sessionID, hash string) {
	b.mu.Lock()
	defer b.mu.Unlock()
	b.getSession(sessionID).turn = hash
}

func (b *snapshotBuiltin) popTurn(sessionID string) string {
	b.mu.Lock()
	defer b.mu.Unlock()
	s := b.session[sessionID]
	if s == nil {
		return ""
	}
	hash := s.turn
	s.turn = ""
	return hash
}

func (b *snapshotBuiltin) setTool(sessionID, toolUseID, hash string) {
	if toolUseID == "" {
		return
	}
	b.mu.Lock()
	defer b.mu.Unlock()
	b.getSession(sessionID).tools[toolUseID] = hash
}

func (b *snapshotBuiltin) popTool(sessionID, toolUseID string) string {
	if toolUseID == "" {
		return ""
	}
	b.mu.Lock()
	defer b.mu.Unlock()
	s := b.session[sessionID]
	if s == nil {
		return ""
	}
	hash := s.tools[toolUseID]
	delete(s.tools, toolUseID)
	return hash
}

func (b *snapshotBuiltin) pushCheckpoint(sessionID string, checkpoint snapshotCheckpoint) {
	if len(checkpoint.files) == 0 {
		return
	}
	b.mu.Lock()
	defer b.mu.Unlock()
	s := b.getSession(sessionID)
	s.history = append(s.history, checkpoint)
}

func (b *snapshotBuiltin) popCheckpoint(sessionID string) (snapshotCheckpoint, bool) {
	b.mu.Lock()
	defer b.mu.Unlock()
	s := b.session[sessionID]
	if s == nil || len(s.history) == 0 {
		return snapshotCheckpoint{}, false
	}
	last := len(s.history) - 1
	checkpoint := s.history[last]
	s.history[last] = snapshotCheckpoint{}
	s.history = s.history[:last]
	return checkpoint, true
}

func (b *snapshotBuiltin) undoLast(ctx context.Context, sessionID, cwd string) (files int, ok bool, err error) {
	checkpoint, ok := b.popCheckpoint(sessionID)
	if !ok {
		return 0, false, nil
	}
	if len(checkpoint.files) == 0 {
		return 0, true, nil
	}
	repo, err := b.manager.Open(ctx, cwd)
	if err != nil {
		return 0, true, err
	}
	patch := snapshot.Patch{Hash: checkpoint.hash, Files: checkpoint.files}
	if err := repo.Revert(ctx, []snapshot.Patch{patch}); err != nil {
		return 0, true, err
	}
	return len(checkpoint.files), true, nil
}

// listSnapshots returns the completed checkpoints for a session in chronological
// order (oldest first). The returned slice may be empty.
func (b *snapshotBuiltin) listSnapshots(sessionID string) []SnapshotInfo {
	b.mu.Lock()
	defer b.mu.Unlock()
	s := b.session[sessionID]
	if s == nil {
		return nil
	}
	out := make([]SnapshotInfo, len(s.history))
	for i, c := range s.history {
		out[i] = SnapshotInfo{Files: len(c.files)}
	}
	return out
}

// resetSnapshot reverts every checkpoint with index >= keep so the workspace
// returns to the state captured at snapshot keep. keep == 0 means "reset to
// the original state". A keep value greater than or equal to the snapshot
// count is a no-op. Reverted checkpoints are dropped from the session history.
func (b *snapshotBuiltin) resetSnapshot(ctx context.Context, sessionID, cwd string, keep int) (files int, ok bool, err error) {
	tail := b.popHistoryTail(sessionID, keep)
	if len(tail) == 0 {
		return 0, false, nil
	}
	repo, err := b.manager.Open(ctx, cwd)
	if err != nil {
		return 0, true, err
	}
	patches := make([]snapshot.Patch, len(tail))
	seen := map[string]bool{}
	for i, c := range tail {
		patches[i] = snapshot.Patch{Hash: c.hash, Files: c.files}
		for _, f := range c.files {
			seen[f] = true
		}
	}
	if err := repo.Revert(ctx, patches); err != nil {
		return 0, true, err
	}
	return len(seen), true, nil
}

// popHistoryTail removes and returns checkpoints with index >= keep, leaving
// the surviving prefix in the session history. keep is clamped to [0, len].
// The popped slots in the backing array are zeroed so the dropped file lists
// can be garbage-collected before the slice grows past them again.
func (b *snapshotBuiltin) popHistoryTail(sessionID string, keep int) []snapshotCheckpoint {
	b.mu.Lock()
	defer b.mu.Unlock()
	s := b.session[sessionID]
	if s == nil {
		return nil
	}
	if keep < 0 {
		keep = 0
	}
	if keep >= len(s.history) {
		return nil
	}
	tail := append([]snapshotCheckpoint(nil), s.history[keep:]...)
	clear(s.history[keep:])
	s.history = s.history[:keep]
	return tail
}

func logPatch(ctx context.Context, scope, sessionID, label string, patch snapshot.Patch, after string) {
	if len(patch.Files) == 0 {
		return
	}
	slog.DebugContext(ctx, "snapshot captured changes",
		"scope", scope,
		"session_id", sessionID,
		"label", label,
		"hash", patch.Hash,
		"after", after,
		"files", len(patch.Files),
	)
}
