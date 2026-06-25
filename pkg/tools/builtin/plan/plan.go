// Package plan provides a toolset that lets two or more agents collaborate on
// plans stored in a global shared folder under the docker-agent data
// directory. Plans are addressed by name, so any agent that loads this toolset
// can write, read, list, and delete the same shared plans, and they persist
// across sessions.
package plan

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"time"

	"github.com/docker/docker-agent/pkg/paths"
	"github.com/docker/docker-agent/pkg/tools"
)

const (
	ToolNameWritePlan  = "write_plan"
	ToolNameReadPlan   = "read_plan"
	ToolNameListPlans  = "list_plans"
	ToolNameDeletePlan = "delete_plan"
)

// Plan is a shared document collaborated on by the agents.
type Plan struct {
	Name    string `json:"name"`
	Title   string `json:"title,omitempty"`
	Content string `json:"content"`
	// Author is a free-form label identifying who last wrote the plan
	// (typically the agent name). It helps collaborators see who made the
	// most recent change.
	Author    string `json:"author,omitempty"`
	Revision  int    `json:"revision"`
	UpdatedAt string `json:"updatedAt"`
}

// Summary is a lightweight view of a plan returned by list_plans.
type Summary struct {
	Name      string `json:"name"`
	Title     string `json:"title,omitempty"`
	Author    string `json:"author,omitempty"`
	Revision  int    `json:"revision"`
	UpdatedAt string `json:"updatedAt"`
}

type ToolSet struct {
	mu  sync.Mutex
	dir string
}

var (
	_ tools.ToolSet      = (*ToolSet)(nil)
	_ tools.Instructable = (*ToolSet)(nil)
	_ tools.Describer    = (*ToolSet)(nil)
)

// CreateToolSet is used by the tools registry.
func CreateToolSet() (tools.ToolSet, error) {
	dir := DefaultDir()
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return nil, fmt.Errorf("failed to create plans directory: %w", err)
	}
	return New(dir), nil
}

// DefaultDir is the global shared folder where plans are stored, under the
// docker-agent data directory.
func DefaultDir() string {
	return filepath.Join(paths.GetDataDir(), "plans")
}

func New(dir string) *ToolSet {
	return &ToolSet{dir: dir}
}

func (t *ToolSet) Describe() string {
	return "plan(dir=" + t.dir + ")"
}

func (t *ToolSet) Instructions() string {
	return `## Plan Tools

Collaborate on shared, named plans with other agents. Plans are stored in a
global shared folder, so every agent using this toolset sees the same plans.

- Use list_plans to discover existing plans.
- Use read_plan to inspect a plan before acting on or changing it.
- Use write_plan to create or update a plan by name. Writing replaces the whole
  document, so read it first and preserve any content you want to keep. Set the
  title and author (your agent name) so collaborators can see who made the
  latest change. Each write bumps the revision number.
- Use delete_plan to remove a plan once it is no longer needed.`
}

// sanitizeName maps a user-supplied plan name to a safe single-segment file
// name. It lowercases, replaces path separators and other illegal characters
// with '-', and collapses to a stable slug so the same name always resolves to
// the same file.
func sanitizeName(name string) string {
	name = strings.TrimSpace(strings.ToLower(name))
	mapped := strings.Map(func(r rune) rune {
		switch {
		case r >= 'a' && r <= 'z', r >= '0' && r <= '9':
			return r
		case r == '-' || r == '_':
			return r
		default:
			return '-'
		}
	}, name)
	// Collapse runs of '-' and trim leading/trailing separators.
	for strings.Contains(mapped, "--") {
		mapped = strings.ReplaceAll(mapped, "--", "-")
	}
	return strings.Trim(mapped, "-")
}

func (t *ToolSet) planPath(name string) (string, error) {
	slug := sanitizeName(name)
	if slug == "" {
		return "", fmt.Errorf("invalid plan name %q", name)
	}
	return filepath.Join(t.dir, slug+".json"), nil
}

func (t *ToolSet) load(path string) (Plan, bool) {
	data, err := os.ReadFile(path)
	if err != nil {
		return Plan{}, false
	}
	var p Plan
	if err := json.Unmarshal(data, &p); err != nil {
		return Plan{}, false
	}
	return p, true
}

func (t *ToolSet) save(path string, p Plan) error {
	if err := os.MkdirAll(t.dir, 0o700); err != nil {
		return fmt.Errorf("creating plans directory: %w", err)
	}
	data, err := json.MarshalIndent(p, "", "  ")
	if err != nil {
		return fmt.Errorf("marshaling plan: %w", err)
	}
	return os.WriteFile(path, data, 0o600)
}

type WritePlanArgs struct {
	Name    string `json:"name" jsonschema:"The plan name (used to address the plan; e.g. 'release', 'migration')."`
	Content string `json:"content" jsonschema:"The full plan content (markdown). Replaces the existing plan."`
	Title   string `json:"title,omitempty" jsonschema:"Optional human-readable title. Preserved from the previous revision when omitted."`
	Author  string `json:"author,omitempty" jsonschema:"Optional label identifying who is writing the plan (typically the agent name)."`
}

func (t *ToolSet) writePlan(_ context.Context, params WritePlanArgs) (*tools.ToolCallResult, error) {
	if params.Content == "" {
		return tools.ResultError("content must not be empty"), nil
	}
	path, err := t.planPath(params.Name)
	if err != nil {
		return tools.ResultError(err.Error()), nil
	}

	t.mu.Lock()
	defer t.mu.Unlock()

	plan, _ := t.load(path)
	plan.Name = sanitizeName(params.Name)
	plan.Content = params.Content
	if params.Title != "" {
		plan.Title = params.Title
	}
	plan.Author = params.Author
	plan.Revision++
	plan.UpdatedAt = time.Now().UTC().Format(time.RFC3339)

	if err := t.save(path, plan); err != nil {
		return tools.ResultError(err.Error()), nil
	}

	return tools.ResultJSON(plan), nil
}

type ReadPlanArgs struct {
	Name string `json:"name" jsonschema:"The name of the plan to read."`
}

func (t *ToolSet) readPlan(_ context.Context, params ReadPlanArgs) (*tools.ToolCallResult, error) {
	path, err := t.planPath(params.Name)
	if err != nil {
		return tools.ResultError(err.Error()), nil
	}

	t.mu.Lock()
	defer t.mu.Unlock()

	plan, ok := t.load(path)
	if !ok {
		return tools.ResultError(fmt.Sprintf("plan %q not found", params.Name)), nil
	}

	return tools.ResultJSON(plan), nil
}

func (t *ToolSet) listPlans(_ context.Context, _ tools.ToolCall) (*tools.ToolCallResult, error) {
	t.mu.Lock()
	defer t.mu.Unlock()

	entries, err := os.ReadDir(t.dir)
	if err != nil {
		if os.IsNotExist(err) {
			return tools.ResultJSON([]Summary{}), nil
		}
		return tools.ResultError(err.Error()), nil
	}

	summaries := make([]Summary, 0, len(entries))
	for _, entry := range entries {
		if entry.IsDir() || filepath.Ext(entry.Name()) != ".json" {
			continue
		}
		plan, ok := t.load(filepath.Join(t.dir, entry.Name()))
		if !ok {
			continue
		}
		summaries = append(summaries, Summary{
			Name:      plan.Name,
			Title:     plan.Title,
			Author:    plan.Author,
			Revision:  plan.Revision,
			UpdatedAt: plan.UpdatedAt,
		})
	}

	sort.Slice(summaries, func(i, j int) bool {
		return summaries[i].Name < summaries[j].Name
	})

	return tools.ResultJSON(summaries), nil
}

type DeletePlanArgs struct {
	Name string `json:"name" jsonschema:"The name of the plan to delete."`
}

func (t *ToolSet) deletePlan(_ context.Context, params DeletePlanArgs) (*tools.ToolCallResult, error) {
	path, err := t.planPath(params.Name)
	if err != nil {
		return tools.ResultError(err.Error()), nil
	}

	t.mu.Lock()
	defer t.mu.Unlock()

	if _, ok := t.load(path); !ok {
		return tools.ResultError(fmt.Sprintf("plan %q not found", params.Name)), nil
	}
	if err := os.Remove(path); err != nil {
		return tools.ResultError(err.Error()), nil
	}

	return tools.ResultJSON(map[string]string{"deleted": sanitizeName(params.Name)}), nil
}

func (t *ToolSet) Tools(context.Context) ([]tools.Tool, error) {
	return []tools.Tool{
		{
			Name:         ToolNameWritePlan,
			Category:     "plan",
			Description:  "Create or update a shared plan by name. Replaces the entire plan content, so read it first to preserve anything you want to keep. Each write bumps the revision number.",
			Parameters:   tools.MustSchemaFor[WritePlanArgs](),
			OutputSchema: tools.MustSchemaFor[Plan](),
			Handler:      tools.NewHandler(t.writePlan),
			Annotations: tools.ToolAnnotations{
				Title: "Write Plan",
			},
		},
		{
			Name:         ToolNameReadPlan,
			Category:     "plan",
			Description:  "Read a shared plan by name, including its title, content, author, and revision number.",
			Parameters:   tools.MustSchemaFor[ReadPlanArgs](),
			OutputSchema: tools.MustSchemaFor[Plan](),
			Handler:      tools.NewHandler(t.readPlan),
			Annotations: tools.ToolAnnotations{
				Title:        "Read Plan",
				ReadOnlyHint: true,
			},
		},
		{
			Name:         ToolNameListPlans,
			Category:     "plan",
			Description:  "List all shared plans with their name, title, author, and revision.",
			OutputSchema: tools.MustSchemaFor[[]Summary](),
			Handler:      t.listPlans,
			Annotations: tools.ToolAnnotations{
				Title:        "List Plans",
				ReadOnlyHint: true,
			},
		},
		{
			Name:        ToolNameDeletePlan,
			Category:    "plan",
			Description: "Delete a shared plan by name.",
			Parameters:  tools.MustSchemaFor[DeletePlanArgs](),
			Handler:     tools.NewHandler(t.deletePlan),
			Annotations: tools.ToolAnnotations{
				Title:           "Delete Plan",
				DestructiveHint: new(true),
			},
		},
	}, nil
}
