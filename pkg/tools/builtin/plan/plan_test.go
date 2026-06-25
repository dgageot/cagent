package plan

import (
	"encoding/json"
	"testing"

	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"

	"github.com/docker/docker-agent/pkg/tools"
)

func newTestPlanTool(t *testing.T) *ToolSet {
	t.Helper()
	return New(t.TempDir())
}

func TestPlanTool_DisplayNames(t *testing.T) {
	tool := newTestPlanTool(t)

	all, err := tool.Tools(t.Context())
	require.NoError(t, err)

	for _, tl := range all {
		assert.NotEmpty(t, tl.DisplayName())
		assert.NotEqual(t, tl.Name, tl.DisplayName())
	}
}

func TestPlanTool_Instructions(t *testing.T) {
	tool := newTestPlanTool(t)
	assert.NotEmpty(t, tool.Instructions())
}

func TestPlanTool_Describe(t *testing.T) {
	tool := newTestPlanTool(t)
	assert.Contains(t, tool.Describe(), "plan(dir=")
}

func TestPlanTool_Write(t *testing.T) {
	tool := newTestPlanTool(t)

	result, err := tool.writePlan(t.Context(), WritePlanArgs{
		Name:    "Release",
		Content: "Step 1: do the thing",
		Title:   "Release plan",
		Author:  "planner",
	})
	require.NoError(t, err)
	assert.False(t, result.IsError)

	var plan Plan
	require.NoError(t, json.Unmarshal([]byte(result.Output), &plan))
	assert.Equal(t, "release", plan.Name)
	assert.Equal(t, "Release plan", plan.Title)
	assert.Equal(t, "Step 1: do the thing", plan.Content)
	assert.Equal(t, "planner", plan.Author)
	assert.Equal(t, 1, plan.Revision)
	assert.NotEmpty(t, plan.UpdatedAt)
}

func TestPlanTool_WriteEmptyContent(t *testing.T) {
	tool := newTestPlanTool(t)

	result, err := tool.writePlan(t.Context(), WritePlanArgs{Name: "p", Content: ""})
	require.NoError(t, err)
	assert.True(t, result.IsError)
	assert.Contains(t, result.Output, "content must not be empty")
}

func TestPlanTool_WriteInvalidName(t *testing.T) {
	tool := newTestPlanTool(t)

	result, err := tool.writePlan(t.Context(), WritePlanArgs{Name: "///", Content: "x"})
	require.NoError(t, err)
	assert.True(t, result.IsError)
	assert.Contains(t, result.Output, "invalid plan name")
}

func TestPlanTool_ReadNotFound(t *testing.T) {
	tool := newTestPlanTool(t)

	result, err := tool.readPlan(t.Context(), ReadPlanArgs{Name: "missing"})
	require.NoError(t, err)
	assert.True(t, result.IsError)
	assert.Contains(t, result.Output, "not found")
}

func TestPlanTool_WriteThenRead(t *testing.T) {
	tool := newTestPlanTool(t)

	_, err := tool.writePlan(t.Context(), WritePlanArgs{Name: "migration", Content: "the plan", Title: "T"})
	require.NoError(t, err)

	result, err := tool.readPlan(t.Context(), ReadPlanArgs{Name: "migration"})
	require.NoError(t, err)
	assert.False(t, result.IsError)

	var plan Plan
	require.NoError(t, json.Unmarshal([]byte(result.Output), &plan))
	assert.Equal(t, "the plan", plan.Content)
	assert.Equal(t, "T", plan.Title)
}

func TestPlanTool_RevisionIncrementsAndTitlePreserved(t *testing.T) {
	tool := newTestPlanTool(t)

	_, err := tool.writePlan(t.Context(), WritePlanArgs{Name: "p", Content: "v1", Title: "Original"})
	require.NoError(t, err)

	// Second write omits the title; it should be preserved.
	result, err := tool.writePlan(t.Context(), WritePlanArgs{Name: "p", Content: "v2"})
	require.NoError(t, err)

	var plan Plan
	require.NoError(t, json.Unmarshal([]byte(result.Output), &plan))
	assert.Equal(t, "v2", plan.Content)
	assert.Equal(t, "Original", plan.Title)
	assert.Equal(t, 2, plan.Revision)
}

func TestPlanTool_ListPlans(t *testing.T) {
	tool := newTestPlanTool(t)

	_, err := tool.writePlan(t.Context(), WritePlanArgs{Name: "beta", Content: "b", Author: "x"})
	require.NoError(t, err)
	_, err = tool.writePlan(t.Context(), WritePlanArgs{Name: "alpha", Content: "a", Author: "y"})
	require.NoError(t, err)

	result, err := tool.listPlans(t.Context(), tools.ToolCall{})
	require.NoError(t, err)
	assert.False(t, result.IsError)

	var summaries []Summary
	require.NoError(t, json.Unmarshal([]byte(result.Output), &summaries))
	require.Len(t, summaries, 2)
	// Sorted by name.
	assert.Equal(t, "alpha", summaries[0].Name)
	assert.Equal(t, "beta", summaries[1].Name)
}

func TestPlanTool_ListEmpty(t *testing.T) {
	tool := newTestPlanTool(t)

	result, err := tool.listPlans(t.Context(), tools.ToolCall{})
	require.NoError(t, err)
	assert.False(t, result.IsError)

	var summaries []Summary
	require.NoError(t, json.Unmarshal([]byte(result.Output), &summaries))
	assert.Empty(t, summaries)
}

func TestPlanTool_DeletePlan(t *testing.T) {
	tool := newTestPlanTool(t)

	_, err := tool.writePlan(t.Context(), WritePlanArgs{Name: "temp", Content: "x"})
	require.NoError(t, err)

	result, err := tool.deletePlan(t.Context(), DeletePlanArgs{Name: "temp"})
	require.NoError(t, err)
	assert.False(t, result.IsError)
	assert.Contains(t, result.Output, "temp")

	// Verify it's gone.
	readResult, err := tool.readPlan(t.Context(), ReadPlanArgs{Name: "temp"})
	require.NoError(t, err)
	assert.True(t, readResult.IsError)
}

func TestPlanTool_DeleteNotFound(t *testing.T) {
	tool := newTestPlanTool(t)

	result, err := tool.deletePlan(t.Context(), DeletePlanArgs{Name: "nope"})
	require.NoError(t, err)
	assert.True(t, result.IsError)
	assert.Contains(t, result.Output, "not found")
}

func TestPlanTool_SharedAcrossInstances(t *testing.T) {
	dir := t.TempDir()

	// One agent writes the plan.
	writer := New(dir)
	_, err := writer.writePlan(t.Context(), WritePlanArgs{
		Name:    "collab",
		Content: "collaborative plan",
		Author:  "agent-a",
	})
	require.NoError(t, err)

	// Another agent, sharing the same folder, reads it.
	reader := New(dir)
	result, err := reader.readPlan(t.Context(), ReadPlanArgs{Name: "collab"})
	require.NoError(t, err)
	assert.False(t, result.IsError)

	var plan Plan
	require.NoError(t, json.Unmarshal([]byte(result.Output), &plan))
	assert.Equal(t, "collaborative plan", plan.Content)
	assert.Equal(t, "agent-a", plan.Author)
}

func TestPlanTool_NameSanitizationIsStable(t *testing.T) {
	tool := newTestPlanTool(t)

	_, err := tool.writePlan(t.Context(), WritePlanArgs{Name: "My Plan!", Content: "v1"})
	require.NoError(t, err)

	// A differently-formatted but equivalent name resolves to the same plan.
	result, err := tool.readPlan(t.Context(), ReadPlanArgs{Name: "my  plan"})
	require.NoError(t, err)
	assert.False(t, result.IsError)

	var plan Plan
	require.NoError(t, json.Unmarshal([]byte(result.Output), &plan))
	assert.Equal(t, "my-plan", plan.Name)
}

func TestPlanTool_ParametersAreObjects(t *testing.T) {
	tool := newTestPlanTool(t)

	allTools, err := tool.Tools(t.Context())
	require.NoError(t, err)
	require.NotEmpty(t, allTools)

	for _, tl := range allTools {
		if tl.Parameters == nil {
			continue
		}
		m, err := tools.SchemaToMap(tl.Parameters)
		require.NoError(t, err)
		assert.Equal(t, "object", m["type"])
	}
}
