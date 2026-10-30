package runtime

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"

	"github.com/docker/docker-agent/pkg/agent"
	"github.com/docker/docker-agent/pkg/cache"
	"github.com/docker/docker-agent/pkg/chat"
	"github.com/docker/docker-agent/pkg/config"
	"github.com/docker/docker-agent/pkg/config/latest"
	"github.com/docker/docker-agent/pkg/environment"
	"github.com/docker/docker-agent/pkg/model/provider"
	"github.com/docker/docker-agent/pkg/model/provider/options"
	"github.com/docker/docker-agent/pkg/session"
	"github.com/docker/docker-agent/pkg/teamloader"
	"github.com/docker/docker-agent/pkg/tools"
	agenttool "github.com/docker/docker-agent/pkg/tools/builtin/agent"
	"github.com/docker/docker-agent/pkg/tools/builtin/filesystem"
	"github.com/docker/docker-agent/pkg/tools/builtin/shell"
	"github.com/docker/docker-agent/pkg/worktree"
)

func worktreeGit(t *testing.T, dir string, args ...string) string {
	t.Helper()
	cmd := exec.CommandContext(t.Context(), "git", append([]string{"-C", dir}, args...)...)
	out, err := cmd.CombinedOutput()
	require.NoError(t, err, string(out))
	return strings.TrimSpace(string(out))
}

func worktreeRepo(t *testing.T) string {
	t.Helper()
	dir := t.TempDir()
	worktreeGit(t, dir, "init")
	worktreeGit(t, dir, "config", "user.name", "Test")
	worktreeGit(t, dir, "config", "user.email", "test@example.com")
	worktreeGit(t, dir, "config", "commit.gpgsign", "false")
	require.NoError(t, os.WriteFile(filepath.Join(dir, "shared.txt"), []byte("base\n"), 0o600))
	worktreeGit(t, dir, "add", ".")
	worktreeGit(t, dir, "commit", "-m", "base")
	return dir
}

func loadWorktreeRuntime(t *testing.T, dir string, models map[string]provider.Provider, hooks string) *LocalRuntime {
	t.Helper()
	source := fmt.Sprintf(`version: "16"
agents:
  root:
    model: openai/root
    sub_agents: [first, second]
    toolsets:
      - type: background_agents
  first:
    model: openai/first
    toolsets:
      - type: shell
      - type: filesystem
%s
  second:
    model: openai/second
    toolsets:
      - type: shell
      - type: filesystem
`, hooks)
	tm, err := teamloader.Load(t.Context(), config.NewBytesSource("test", []byte(source)), &config.RuntimeConfig{
		Config:              config.Config{WorkingDir: dir},
		EnvProviderForTests: environment.NewMapEnvProvider(map[string]string{"OPENAI_API_KEY": "test"}),
	}, teamloader.WithProviderRegistry(provider.NewRegistry(map[string]provider.Factory{
		"openai": func(_ context.Context, cfg *latest.ModelConfig, _ environment.Provider, _ ...options.Opt) (provider.Provider, error) {
			return models[cfg.Model], nil
		},
	})), teamloader.WithToolsetRegistry(teamloader.NewToolsetRegistry(map[string]teamloader.ToolsetCreator{
		"shell":             shell.Creator,
		"filesystem":        filesystem.Creator,
		"background_agents": teamloader.Creator(agenttool.CreateToolSet),
	})))
	require.NoError(t, err)
	rt, err := NewLocalRuntime(t.Context(), tm, WithWorkingDir(dir), WithSessionCompaction(false), WithModelStore(mockModelStore{}))
	require.NoError(t, err)
	t.Cleanup(func() { require.NoError(t, rt.Close()); require.NoError(t, tm.StopToolSets(t.Context())) })
	return rt
}

func writingWorktreeModel(name string) *queueProvider {
	return &queueProvider{id: "test/mock-model", streams: []chat.MessageStream{
		newStreamBuilder().AddToolCallName("write", "write_file").
			AddToolCallArguments("write", fmt.Sprintf(`{"path":%q,"content":%q}`, name+".txt", name)).AddToolCallStopWithUsage(10, 5).Build(),
		newStreamBuilder().AddToolCallName("commit", "shell").
			AddToolCallArguments("commit", fmt.Sprintf(`{"cmd":"git add %s.txt && git commit -m %s"}`, name, name)).AddToolCallStopWithUsage(10, 5).Build(),
		newStreamBuilder().AddContent("finished "+name).AddStopWithUsage(10, 5).Build(),
	}}
}

func TestBackgroundWorktreeIsolationAndMerge(t *testing.T) {
	for _, linked := range []bool{false, true} {
		t.Run(fmt.Sprintf("linked=%t", linked), func(t *testing.T) {
			t.Parallel()
			dir := worktreeRepo(t)
			if linked {
				linkedDir := filepath.Join(t.TempDir(), "parent")
				worktreeGit(t, dir, "worktree", "add", "-b", "parent", linkedDir)
				dir = linkedDir
				require.NoError(t, os.WriteFile(filepath.Join(dir, "parent.txt"), []byte("parent"), 0o600))
				worktreeGit(t, dir, "add", ".")
				worktreeGit(t, dir, "commit", "-m", "parent only")
			}
			base := worktreeGit(t, dir, "rev-parse", "HEAD")
			require.NoError(t, os.WriteFile(filepath.Join(dir, "shared.txt"), []byte("uncommitted\n"), 0o600))
			rt := loadWorktreeRuntime(t, dir, map[string]provider.Provider{
				"root":   &mockProvider{id: "test/mock-model"},
				"first":  writingWorktreeModel("first"),
				"second": writingWorktreeModel("second"),
			}, "")
			parent := session.New(session.WithWorkingDir(dir), session.WithToolsApproved(true))
			ids := make([]string, 0, 2)
			for _, name := range []string{"first", "second"} {
				result, err := rt.bgAgents.HandleRun(t.Context(), parent, tools.ToolCall{Function: tools.FunctionCall{Arguments: fmt.Sprintf(`{"agent":%q,"task":"implement your file","worktree":true}`, name)}})
				require.NoError(t, err)
				require.False(t, result.IsError, result.Output)
				ids = append(ids, parseBackgroundTaskID(t, result.Output))
			}
			args, _ := json.Marshal(agenttool.WaitBackgroundAgentsArgs{TaskIDs: ids, Timeout: 30})
			result, err := rt.bgAgents.HandleWait(t.Context(), parent, tools.ToolCall{Function: tools.FunctionCall{Arguments: string(args)}})
			require.NoError(t, err)
			var joined struct {
				AllDone bool `json:"all_done"`
				Tasks   []struct {
					Status, Output, Error, Worktree, Branch string
					Base                                    string `json:"base_commit"`
				} `json:"tasks"`
			}
			require.NoError(t, json.Unmarshal([]byte(result.Output), &joined))
			require.True(t, joined.AllDone, result.Output)
			require.Len(t, joined.Tasks, 2)
			assert.NotEqual(t, joined.Tasks[0].Worktree, joined.Tasks[1].Worktree)
			for i, task := range joined.Tasks {
				name := []string{"first", "second"}[i]
				require.Equal(t, "completed", task.Status, result.Output)
				require.NotEmpty(t, task.Worktree)
				wt := &worktree.Worktree{Dir: task.Worktree, Branch: task.Branch, SourceDir: dir}
				t.Cleanup(func() { require.NoError(t, wt.Remove(context.WithoutCancel(t.Context()))) })
				assert.Equal(t, base, task.Base)
				assert.Equal(t, "base", stringReadFile(t, filepath.Join(task.Worktree, "shared.txt")))
				assert.Equal(t, name, stringReadFile(t, filepath.Join(task.Worktree, name+".txt")))
				if linked {
					assert.FileExists(t, filepath.Join(task.Worktree, "parent.txt"))
				}
				assert.NoFileExists(t, filepath.Join(dir, name+".txt"))
				assert.Empty(t, worktreeGit(t, task.Worktree, "status", "--porcelain"))
				assert.NotEqual(t, base, worktreeGit(t, task.Worktree, "rev-parse", "HEAD"))
			}
			assert.Equal(t, base, worktreeGit(t, dir, "rev-parse", "HEAD"))
			assert.Equal(t, "uncommitted", stringReadFile(t, filepath.Join(dir, "shared.txt")))
			for _, task := range joined.Tasks {
				worktreeGit(t, dir, "merge", "--no-edit", task.Branch)
			}
			assert.FileExists(t, filepath.Join(dir, "first.txt"))
			assert.FileExists(t, filepath.Join(dir, "second.txt"))
			assert.Equal(t, "uncommitted", stringReadFile(t, filepath.Join(dir, "shared.txt")))
			var children []*session.Session
			for _, item := range parent.Messages {
				if item.SubSession != nil {
					children = append(children, item.SubSession)
				}
			}
			require.Len(t, children, 2)
			for _, child := range children {
				assert.NotEqual(t, dir, child.WorkingDir)
				assert.Equal(t, parent.ID, child.ParentID)
				assert.NotEmpty(t, child.AttributesSnapshot()["worktree.branch"])
			}
		})
	}
}

func stringReadFile(t *testing.T, path string) string {
	t.Helper()
	data, err := os.ReadFile(path)
	require.NoError(t, err)
	return strings.TrimSpace(string(data))
}

func TestBackgroundWorktreeFailuresRetainCheckout(t *testing.T) {
	t.Parallel()
	for _, mode := range []string{"model error", "blocked hook"} {
		t.Run(mode, func(t *testing.T) {
			t.Parallel()
			dir := worktreeRepo(t)
			hooks := ""
			if mode == "blocked hook" {
				hooks = "    hooks:\n      worktree_create:\n        - type: command\n          command: 'echo hook > prepared.txt; exit 2'"
			}
			rt := loadWorktreeRuntime(t, dir, map[string]provider.Provider{
				"root":   &mockProvider{id: "test/mock-model"},
				"first":  &mockProviderWithError{id: "test/mock-model"},
				"second": &mockProvider{id: "test/mock-model"},
			}, hooks)
			var wt *worktree.Worktree
			result := rt.RunAgent(t.Context(), agenttool.RunParams{AgentName: "first", Task: "do work", Worktree: true, ParentSession: session.New(session.WithWorkingDir(dir)), OnWorktree: func(w *worktree.Worktree) { wt = w }})
			require.NotEmpty(t, result.ErrMsg)
			require.NotNil(t, wt)
			t.Cleanup(func() { require.NoError(t, wt.Remove(context.WithoutCancel(t.Context()))) })
			assert.DirExists(t, wt.Dir)
			if mode == "blocked hook" {
				assert.FileExists(t, filepath.Join(wt.Dir, "prepared.txt"))
				assert.NoFileExists(t, filepath.Join(dir, "prepared.txt"))
			}
		})
	}
}

func TestBackgroundWorktreeRequiresRepository(t *testing.T) {
	t.Parallel()
	dir := t.TempDir()
	rt := loadWorktreeRuntime(t, dir, map[string]provider.Provider{"root": &mockProvider{}, "first": &mockProvider{}, "second": &mockProvider{}}, "")
	for _, workingDir := range []string{"", dir} {
		result := rt.RunAgent(t.Context(), agenttool.RunParams{AgentName: "first", Task: "do work", Worktree: true, ParentSession: session.New(session.WithWorkingDir(workingDir))})
		require.NotEmpty(t, result.ErrMsg)
		assert.Contains(t, result.ErrMsg, "creating background worktree")
	}
}

func TestBackgroundWorktreeCancellation(t *testing.T) {
	t.Parallel()
	dir := worktreeRepo(t)
	entered := make(chan struct{})
	release := make(chan struct{})
	model := &worktreeBlockingProvider{entered: entered, release: release}
	rt := loadWorktreeRuntime(t, dir, map[string]provider.Provider{
		"root": &mockProvider{id: "test/mock-model"}, "first": model, "second": &mockProvider{id: "test/mock-model"},
	}, "")
	ctx, cancel := context.WithCancel(t.Context())
	defer cancel()
	parent := session.New(session.WithWorkingDir(dir))
	created := make(chan *worktree.Worktree, 1)
	done := make(chan *agenttool.RunResult, 1)
	go func() {
		done <- rt.RunAgent(ctx, agenttool.RunParams{
			AgentName: "first", Task: "wait", Worktree: true,
			ParentSession: parent,
			OnWorktree:    func(wt *worktree.Worktree) { created <- wt },
		})
	}()
	wt := <-created
	t.Cleanup(func() { require.NoError(t, wt.Remove(context.WithoutCancel(t.Context()))) })
	<-entered
	rows := rt.LiveSessions(t.Context(), parent)
	require.Len(t, rows, 2)
	assert.Equal(t, "first", rows[1].AgentName)
	require.NoError(t, rt.CompactLiveSession(t.Context(), rows[1].SessionID, "", nil))
	cancel()
	close(release)
	<-done
	assert.Len(t, rt.LiveSessions(t.Context(), parent), 1)
	assert.DirExists(t, wt.Dir)
	assert.Equal(t, wt.BaseCommit, worktreeGit(t, wt.Dir, "rev-parse", "HEAD"))
}

type worktreeBlockingProvider struct {
	mockProvider

	entered chan struct{}
	release chan struct{}
}

func (p *worktreeBlockingProvider) CreateChatCompletionStream(ctx context.Context, _ []chat.Message, _ []tools.Tool) (chat.MessageStream, error) {
	close(p.entered)
	select {
	case <-ctx.Done():
		return nil, ctx.Err()
	case <-p.release:
		return newStreamBuilder().AddContent("released").AddStopWithUsage(1, 1).Build(), nil
	}
}

func TestBackgroundWorktreeRuntimeState(t *testing.T) {
	t.Parallel()
	dir := worktreeRepo(t)
	rt := loadWorktreeRuntime(t, dir, map[string]provider.Provider{
		"root": &mockProvider{id: "test/mock-model"}, "first": &mockProvider{id: "test/mock-model"}, "second": &mockProvider{id: "test/mock-model"},
	}, "")
	rt.budgetCfg = &latest.BudgetConfig{MaxTokens: 100}
	originalAgent, _ := rt.team.Agent("first")
	responseCache, err := cache.New(cache.Config{Enabled: true})
	require.NoError(t, err)
	responseCache.Store("Please proceed.", "cached")
	agent.WithCache(responseCache)(originalAgent)
	parent := session.New(session.WithWorkingDir(dir), session.WithSafetyPolicy(session.SafetyPolicyStrict))
	wt, child, err := rt.prepareBackgroundWorktree(t.Context(), agenttool.RunParams{AgentName: "first", ParentSession: parent})
	require.NoError(t, err)
	t.Cleanup(func() {
		closeWorktreeRuntime(context.WithoutCancel(t.Context()), child)
		require.NoError(t, wt.Remove(context.WithoutCancel(t.Context())))
	})
	assert.Same(t, rt.currentBudget(), child.currentBudget())
	assert.Nil(t, child.CurrentAgent().Cache())
	assert.Same(t, responseCache, originalAgent.Cache())
	nested := child.RunAgent(t.Context(), agenttool.RunParams{})
	assert.Contains(t, nested.ErrMsg, "nested background agents")
	assert.NotSame(t, rt.hooksRegistry, child.hooksRegistry)
	assert.NotSame(t, rt.steerQueue, child.steerQueue)
	assert.Equal(t, wt.Dir, child.workingDir)
	assert.Equal(t, dir, rt.workingDir)
	harnessAgent := agent.New("harness", "", agent.WithHarness(&latest.HarnessConfig{Type: "codex"}))
	events := make(chan Event, 1)
	assert.Equal(t, turnEndReasonError, child.runHarnessAgent(t.Context(), session.New(), harnessAgent, NewChannelSink(events)))
	assert.IsType(t, &ErrorEvent{}, <-events)
	assert.Equal(t, "root", rt.currentAgentName())
	assert.Equal(t, "first", child.currentAgentName())
	sess := newSubSession(parent, SubSessionConfig{AgentName: "first", Task: "edit " + filepath.Join(dir, "shared.txt"), Worktree: wt, SafetyPolicy: parent.GetSafetyPolicy()}, child.CurrentAgent())
	assert.Equal(t, session.SafetyPolicyStrict, sess.GetSafetyPolicy())
	var transcript strings.Builder
	for _, item := range sess.Messages {
		if item.Message != nil {
			transcript.WriteString(item.Message.Message.Content)
		}
	}
	assert.Contains(t, transcript.String(), filepath.Join(wt.Dir, "shared.txt"))
}
