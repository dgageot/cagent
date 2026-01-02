package codemode

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"testing"

	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"

	"github.com/docker/cagent/pkg/config"
	"github.com/docker/cagent/pkg/memory/database"
	"github.com/docker/cagent/pkg/tools"
	"github.com/docker/cagent/pkg/tools/builtin"
)

func TestCodeModeTool_Tools(t *testing.T) {
	tool := &codeModeTool{}

	toolSet, err := tool.Tools(t.Context())
	require.NoError(t, err)
	require.Len(t, toolSet, 1)

	fetchTool := toolSet[0]
	assert.Equal(t, "run_tools_with_javascript", fetchTool.Name)
	assert.Equal(t, "code mode", fetchTool.Category)
	assert.NotNil(t, fetchTool.Handler)

	inputSchema, err := json.Marshal(fetchTool.Parameters)
	require.NoError(t, err)
	assert.JSONEq(t, `{
	"type": "object",
	"required": [
		"script"
	],
	"properties": {
		"script": {
			"type": "string",
			"description": "Script to execute"
		}
	},
	"additionalProperties": false
}`, string(inputSchema))

	outputSchema, err := json.Marshal(fetchTool.OutputSchema)
	require.NoError(t, err)
	assert.JSONEq(t, `{
	"type": "object",
	"required": [
		"value",
		"stdout",
		"stderr"
	],
	"properties": {
		"stderr": {
			"type": "string",
			"description": "The standard error of the console"
		},
		"stdout": {
			"type": "string",
			"description": "The standard output of the console"
		},
		"value": {
			"type": "string",
			"description": "The value returned by the script"
		},
		"tool_calls": {
			"type": "array",
			"description": "The list of tool calls made during script execution, only included on failure",
			"items": {
				"type": "object",
				"additionalProperties": false,
				"required": ["name", "arguments"],
				"properties": {
					"name": {
						"type": "string",
						"description": "The name of the tool that was called"
					},
					"arguments": {
						"description": "The arguments passed to the tool"
					},
					"result": {
						"type": "string",
						"description": "The raw response returned by the tool"
					},
					"error": {
						"type": "string",
						"description": "The error message, if the tool call failed"
					}
				}
			}
		}
	},
	"additionalProperties": false
}`, string(outputSchema))
}

func TestCodeModeTool_Instructions(t *testing.T) {
	tool := &codeModeTool{}

	instructions := tool.Instructions()

	assert.Empty(t, instructions)
}

func TestCodeModeTool_StartStop(t *testing.T) {
	inner := &testToolSet{}

	tool := Wrap(inner)

	assert.Equal(t, 0, inner.start)
	assert.Equal(t, 0, inner.stop)

	err := tool.Start(t.Context())
	require.NoError(t, err)
	assert.Equal(t, 1, inner.start)
	assert.Equal(t, 0, inner.stop)

	err = tool.Stop(t.Context())
	require.NoError(t, err)
	assert.Equal(t, 1, inner.start)
	assert.Equal(t, 1, inner.stop)
}

func TestCodeModeTool_CallHello(t *testing.T) {
	tool := Wrap(&testToolSet{
		tools: []tools.Tool{
			{
				Name: "hello_world",
				Handler: builtin.NewHandler(func(ctx context.Context, args map[string]any) (*tools.ToolCallResult, error) {
					return tools.ResultSuccess("Hello, World!"), nil
				}),
			},
		},
	})

	allTools, err := tool.Tools(t.Context())
	require.NoError(t, err)
	require.Len(t, allTools, 1)

	result, err := allTools[0].Handler(t.Context(), tools.ToolCall{
		Function: tools.FunctionCall{
			Arguments: `{"script":"return hello_world();"}`,
		},
	})
	require.NoError(t, err)

	var scriptResult ScriptResult
	err = json.Unmarshal([]byte(result.Output), &scriptResult)
	require.NoError(t, err)

	require.Equal(t, "Hello, World!", scriptResult.Value)
	require.Empty(t, scriptResult.StdErr)
	require.Empty(t, scriptResult.StdOut)
}

func TestCodeModeTool_CallEcho(t *testing.T) {
	type EchoArgs struct {
		Message string `json:"message" jsonschema:"Message to echo"`
	}

	tool := Wrap(&testToolSet{
		tools: []tools.Tool{{
			Name: "echo",
			Handler: builtin.NewHandler(func(ctx context.Context, args map[string]any) (*tools.ToolCallResult, error) {
				return tools.ResultSuccess("ECHO"), nil
			}),
			Parameters: tools.MustSchemaFor[EchoArgs](),
		}},
	})

	allTools, err := tool.Tools(t.Context())
	require.NoError(t, err)
	require.Len(t, allTools, 1)

	result, err := allTools[0].Handler(t.Context(), tools.ToolCall{
		Function: tools.FunctionCall{
			Arguments: `{"script":"return echo({'message':'ECHO'});"}`,
		},
	})
	require.NoError(t, err)

	var scriptResult ScriptResult
	err = json.Unmarshal([]byte(result.Output), &scriptResult)
	require.NoError(t, err)

	require.Equal(t, "ECHO", scriptResult.Value)
	require.Empty(t, scriptResult.StdErr)
	require.Empty(t, scriptResult.StdOut)
}

func TestCodeModeTool_StructuredOutputForFilesystem(t *testing.T) {
	// Create a temp directory with some files
	tmpDir := t.TempDir()
	require.NoError(t, os.WriteFile(filepath.Join(tmpDir, "file1.txt"), []byte("content1"), 0o644))
	require.NoError(t, os.WriteFile(filepath.Join(tmpDir, "file2.txt"), []byte("content2"), 0o644))
	require.NoError(t, os.Mkdir(filepath.Join(tmpDir, "subdir"), 0o755))

	// Create filesystem tool and wrap in code mode
	fs := builtin.NewFilesystemTool([]string{tmpDir})
	tool := Wrap(fs)

	allTools, err := tool.Tools(t.Context())
	require.NoError(t, err)
	require.NotEmpty(t, allTools)

	// Find the code mode tool (run_tools_with_javascript)
	var codeModeHandler tools.ToolHandler
	for _, t := range allTools {
		if t.Name == "run_tools_with_javascript" {
			codeModeHandler = t.Handler
			break
		}
	}
	require.NotNil(t, codeModeHandler)

	// Test list_directory returns structured JSON in Code Mode
	result, err := codeModeHandler(t.Context(), tools.ToolCall{
		Function: tools.FunctionCall{
			Arguments: fmt.Sprintf(`{"script":"return list_directory({'path': '%s'});"}`, tmpDir),
		},
	})
	require.NoError(t, err)

	var scriptResult ScriptResult
	err = json.Unmarshal([]byte(result.Output), &scriptResult)
	require.NoError(t, err)

	// The value should be JSON that we can parse
	var listDirResult builtin.ListDirectoryMeta
	err = json.Unmarshal([]byte(scriptResult.Value), &listDirResult)
	require.NoError(t, err, "list_directory should return structured JSON in Code Mode")

	assert.ElementsMatch(t, []string{"file1.txt", "file2.txt"}, listDirResult.Files)
	assert.ElementsMatch(t, []string{"subdir"}, listDirResult.Dirs)
}

func TestCodeModeTool_StructuredOutputForReadFile(t *testing.T) {
	// Create a temp file
	tmpDir := t.TempDir()
	filePath := filepath.Join(tmpDir, "test.txt")
	require.NoError(t, os.WriteFile(filePath, []byte("line1\nline2\nline3"), 0o644))

	// Create filesystem tool and wrap in code mode
	fs := builtin.NewFilesystemTool([]string{tmpDir})
	tool := Wrap(fs)

	allTools, err := tool.Tools(t.Context())
	require.NoError(t, err)
	require.NotEmpty(t, allTools)

	// Find the code mode tool (run_tools_with_javascript)
	var codeModeHandler tools.ToolHandler
	for _, tt := range allTools {
		if tt.Name == "run_tools_with_javascript" {
			codeModeHandler = tt.Handler
			break
		}
	}
	require.NotNil(t, codeModeHandler)

	// Test read_file returns structured JSON in Code Mode
	result, err := codeModeHandler(t.Context(), tools.ToolCall{
		Function: tools.FunctionCall{
			Arguments: fmt.Sprintf(`{"script":"return read_file({'path': '%s'});"}`, filePath),
		},
	})
	require.NoError(t, err)

	var scriptResult ScriptResult
	err = json.Unmarshal([]byte(result.Output), &scriptResult)
	require.NoError(t, err)

	// The value should be JSON that we can parse
	var readFileResult builtin.ReadFileMeta
	err = json.Unmarshal([]byte(scriptResult.Value), &readFileResult)
	require.NoError(t, err, "read_file should return structured JSON in Code Mode")

	assert.Equal(t, filePath, readFileResult.Path)
	assert.Equal(t, "line1\nline2\nline3", readFileResult.Content)
	assert.Equal(t, 3, readFileResult.LineCount)
}

func TestCodeModeTool_StructuredOutputForTodo(t *testing.T) {
	// Create todo tool and wrap in code mode
	todoTool := builtin.NewTodoTool()
	tool := Wrap(todoTool)

	allTools, err := tool.Tools(t.Context())
	require.NoError(t, err)
	require.NotEmpty(t, allTools)

	// Find the code mode tool (run_tools_with_javascript)
	var codeModeHandler tools.ToolHandler
	for _, t := range allTools {
		if t.Name == "run_tools_with_javascript" {
			codeModeHandler = t.Handler
			break
		}
	}
	require.NotNil(t, codeModeHandler)

	// Create a todo and verify structured output
	result, err := codeModeHandler(t.Context(), tools.ToolCall{
		Function: tools.FunctionCall{
			Arguments: `{"script":"return create_todo({'description': 'Test task'});"}`,
		},
	})
	require.NoError(t, err)

	var scriptResult ScriptResult
	err = json.Unmarshal([]byte(result.Output), &scriptResult)
	require.NoError(t, err)

	// The value should be JSON array of todos
	var todos []builtin.Todo
	err = json.Unmarshal([]byte(scriptResult.Value), &todos)
	require.NoError(t, err, "create_todo should return structured JSON in Code Mode")

	require.Len(t, todos, 1)
	assert.Equal(t, "todo_1", todos[0].ID)
	assert.Equal(t, "Test task", todos[0].Description)
	assert.Equal(t, "pending", todos[0].Status)

	// List todos and verify structured output
	result, err = codeModeHandler(t.Context(), tools.ToolCall{
		Function: tools.FunctionCall{
			Arguments: `{"script":"return list_todos();"}`,
		},
	})
	require.NoError(t, err)

	err = json.Unmarshal([]byte(result.Output), &scriptResult)
	require.NoError(t, err)

	err = json.Unmarshal([]byte(scriptResult.Value), &todos)
	require.NoError(t, err, "list_todos should return structured JSON in Code Mode")

	require.Len(t, todos, 1)
	assert.Equal(t, "Test task", todos[0].Description)
}

type testToolSet struct {
	tools.BaseToolSet

	tools []tools.Tool
	start int
	stop  int
}

func (t *testToolSet) Tools(context.Context) ([]tools.Tool, error) {
	return t.tools, nil
}

func (t *testToolSet) Start(context.Context) error {
	t.start++
	return nil
}

func (t *testToolSet) Stop(context.Context) error {
	t.stop++
	return nil
}

// TestCodeModeTool_SuccessNoToolCalls verifies that successful execution does not include tool calls.
func TestCodeModeTool_SuccessNoToolCalls(t *testing.T) {
	tool := Wrap(&testToolSet{
		tools: []tools.Tool{
			{
				Name: "get_data",
				Handler: builtin.NewHandler(func(ctx context.Context, args map[string]any) (*tools.ToolCallResult, error) {
					return tools.ResultSuccess("data"), nil
				}),
			},
		},
	})

	allTools, err := tool.Tools(t.Context())
	require.NoError(t, err)
	require.Len(t, allTools, 1)

	result, err := allTools[0].Handler(t.Context(), tools.ToolCall{
		Function: tools.FunctionCall{
			Arguments: `{"script":"return get_data();"}`,
		},
	})
	require.NoError(t, err)

	var scriptResult ScriptResult
	err = json.Unmarshal([]byte(result.Output), &scriptResult)
	require.NoError(t, err)

	// Success case should not include tool calls
	assert.Equal(t, "data", scriptResult.Value)
	assert.Empty(t, scriptResult.ToolCalls, "successful execution should not include tool_calls")
}

// TestCodeModeTool_FailureIncludesToolCalls verifies that failed execution includes tool call history.
func TestCodeModeTool_FailureIncludesToolCalls(t *testing.T) {
	tool := Wrap(&testToolSet{
		tools: []tools.Tool{
			{
				Name: "first_tool",
				Handler: builtin.NewHandler(func(ctx context.Context, args map[string]any) (*tools.ToolCallResult, error) {
					return tools.ResultSuccess("first result"), nil
				}),
			},
			{
				Name: "second_tool",
				Handler: builtin.NewHandler(func(ctx context.Context, args map[string]any) (*tools.ToolCallResult, error) {
					return tools.ResultSuccess("second result"), nil
				}),
			},
		},
	})

	allTools, err := tool.Tools(t.Context())
	require.NoError(t, err)
	require.Len(t, allTools, 1)

	// Script calls tools successfully but then throws a runtime error
	result, err := allTools[0].Handler(t.Context(), tools.ToolCall{
		Function: tools.FunctionCall{
			Arguments: `{"script":"var a = first_tool(); var b = second_tool(); throw new Error('runtime error');"}`,
		},
	})
	require.NoError(t, err)

	var scriptResult ScriptResult
	err = json.Unmarshal([]byte(result.Output), &scriptResult)
	require.NoError(t, err)

	// Failure case should include tool calls
	assert.Contains(t, scriptResult.Value, "runtime error")
	require.Len(t, scriptResult.ToolCalls, 2, "failed execution should include tool_calls")

	// Verify first tool call
	assert.Equal(t, "first_tool", scriptResult.ToolCalls[0].Name)
	assert.Equal(t, "first result", scriptResult.ToolCalls[0].Result)
	assert.Empty(t, scriptResult.ToolCalls[0].Error)

	// Verify second tool call
	assert.Equal(t, "second_tool", scriptResult.ToolCalls[1].Name)
	assert.Equal(t, "second result", scriptResult.ToolCalls[1].Result)
	assert.Empty(t, scriptResult.ToolCalls[1].Error)
}

// TestCodeModeTool_FailureIncludesToolError verifies that tool errors are captured in tool call history.
func TestCodeModeTool_FailureIncludesToolError(t *testing.T) {
	tool := Wrap(&testToolSet{
		tools: []tools.Tool{
			{
				Name: "failing_tool",
				Handler: builtin.NewHandler(func(ctx context.Context, args map[string]any) (*tools.ToolCallResult, error) {
					return nil, assert.AnError
				}),
			},
		},
	})

	allTools, err := tool.Tools(t.Context())
	require.NoError(t, err)
	require.Len(t, allTools, 1)

	result, err := allTools[0].Handler(t.Context(), tools.ToolCall{
		Function: tools.FunctionCall{
			Arguments: `{"script":"return failing_tool();"}`,
		},
	})
	require.NoError(t, err)

	var scriptResult ScriptResult
	err = json.Unmarshal([]byte(result.Output), &scriptResult)
	require.NoError(t, err)

	// Script fails due to tool error
	assert.Contains(t, scriptResult.Value, "assert.AnError")
	require.Len(t, scriptResult.ToolCalls, 1, "failed execution should include tool_calls")

	// Verify the tool call recorded the error
	assert.Equal(t, "failing_tool", scriptResult.ToolCalls[0].Name)
	assert.Empty(t, scriptResult.ToolCalls[0].Result)
	assert.Contains(t, scriptResult.ToolCalls[0].Error, "assert.AnError")
}

// TestCodeModeTool_FailureIncludesToolArguments verifies that tool arguments are captured.
func TestCodeModeTool_FailureIncludesToolArguments(t *testing.T) {
	type TestArgs struct {
		Value string `json:"value" jsonschema:"Test value"`
	}

	tool := Wrap(&testToolSet{
		tools: []tools.Tool{
			{
				Name: "tool_with_args",
				Handler: builtin.NewHandler(func(ctx context.Context, args map[string]any) (*tools.ToolCallResult, error) {
					return tools.ResultSuccess("result"), nil
				}),
				Parameters: tools.MustSchemaFor[TestArgs](),
			},
		},
	})

	allTools, err := tool.Tools(t.Context())
	require.NoError(t, err)
	require.Len(t, allTools, 1)

	result, err := allTools[0].Handler(t.Context(), tools.ToolCall{
		Function: tools.FunctionCall{
			Arguments: `{"script":"tool_with_args({'value': 'test123'}); throw new Error('forced error');"}`,
		},
	})
	require.NoError(t, err)

	var scriptResult ScriptResult
	err = json.Unmarshal([]byte(result.Output), &scriptResult)
	require.NoError(t, err)

	// Verify the tool call captured the arguments
	require.Len(t, scriptResult.ToolCalls, 1)
	assert.Equal(t, "tool_with_args", scriptResult.ToolCalls[0].Name)
	assert.Equal(t, map[string]any{"value": "test123"}, scriptResult.ToolCalls[0].Arguments)
	assert.Equal(t, "result", scriptResult.ToolCalls[0].Result)
}

func TestCodeModeTool_StructuredOutputForMemory(t *testing.T) {
	// Create a mock memory database
	mockDB := &mockMemoryDB{
		memories: make(map[string]mockMemory),
	}

	// Create memory tool and wrap in code mode
	memoryTool := builtin.NewMemoryTool(mockDB)
	tool := Wrap(memoryTool)

	allTools, err := tool.Tools(t.Context())
	require.NoError(t, err)
	require.NotEmpty(t, allTools)

	// Find the code mode tool
	var codeModeHandler tools.ToolHandler
	for _, tt := range allTools {
		if tt.Name == "run_tools_with_javascript" {
			codeModeHandler = tt.Handler
			break
		}
	}
	require.NotNil(t, codeModeHandler)

	// Add a memory and verify structured output
	result, err := codeModeHandler(t.Context(), tools.ToolCall{
		Function: tools.FunctionCall{
			Arguments: `{"script":"return add_memory({'memory': 'Test memory'});"}`,
		},
	})
	require.NoError(t, err)

	var scriptResult ScriptResult
	err = json.Unmarshal([]byte(result.Output), &scriptResult)
	require.NoError(t, err)

	// The value should be JSON array of memories
	var memories []mockMemory
	err = json.Unmarshal([]byte(scriptResult.Value), &memories)
	require.NoError(t, err, "add_memory should return structured JSON in Code Mode")

	require.Len(t, memories, 1)
	assert.Equal(t, "Test memory", memories[0].Memory)

	// Get memories and verify structured output
	result, err = codeModeHandler(t.Context(), tools.ToolCall{
		Function: tools.FunctionCall{
			Arguments: `{"script":"return get_memories();"}`,
		},
	})
	require.NoError(t, err)

	err = json.Unmarshal([]byte(result.Output), &scriptResult)
	require.NoError(t, err)

	err = json.Unmarshal([]byte(scriptResult.Value), &memories)
	require.NoError(t, err, "get_memories should return structured JSON in Code Mode")

	require.Len(t, memories, 1)
	assert.Equal(t, "Test memory", memories[0].Memory)
}

func TestCodeModeTool_StructuredOutputForBackgroundJobs(t *testing.T) {
	// Create shell tool and wrap in code mode
	shellTool := builtin.NewShellTool(nil, &config.RuntimeConfig{Config: config.Config{WorkingDir: t.TempDir()}})
	tool := Wrap(shellTool)
	defer func() { _ = shellTool.Stop(t.Context()) }()

	allTools, err := tool.Tools(t.Context())
	require.NoError(t, err)
	require.NotEmpty(t, allTools)

	// Find the code mode tool
	var codeModeHandler tools.ToolHandler
	for _, tt := range allTools {
		if tt.Name == "run_tools_with_javascript" {
			codeModeHandler = tt.Handler
			break
		}
	}
	require.NotNil(t, codeModeHandler)

	// List background jobs (empty) and verify structured output
	result, err := codeModeHandler(t.Context(), tools.ToolCall{
		Function: tools.FunctionCall{
			Arguments: `{"script":"return list_background_jobs();"}`,
		},
	})
	require.NoError(t, err)

	var scriptResult ScriptResult
	err = json.Unmarshal([]byte(result.Output), &scriptResult)
	require.NoError(t, err)

	// The value should be JSON array of jobs (empty)
	var jobs []builtin.BackgroundJobInfo
	err = json.Unmarshal([]byte(scriptResult.Value), &jobs)
	require.NoError(t, err, "list_background_jobs should return structured JSON in Code Mode")
	assert.Empty(t, jobs)
}

// Mock memory database for testing
type mockMemory struct {
	ID        string `json:"id"`
	CreatedAt string `json:"created_at"`
	Memory    string `json:"memory"`
}

type mockMemoryDB struct {
	memories map[string]mockMemory
}

func (m *mockMemoryDB) AddMemory(_ context.Context, memory database.UserMemory) error {
	m.memories[memory.ID] = mockMemory{
		ID:        memory.ID,
		CreatedAt: memory.CreatedAt,
		Memory:    memory.Memory,
	}
	return nil
}

func (m *mockMemoryDB) GetMemories(_ context.Context) ([]database.UserMemory, error) {
	var result []database.UserMemory
	for _, mem := range m.memories {
		result = append(result, database.UserMemory{
			ID:        mem.ID,
			CreatedAt: mem.CreatedAt,
			Memory:    mem.Memory,
		})
	}
	return result, nil
}

func (m *mockMemoryDB) DeleteMemory(_ context.Context, memory database.UserMemory) error {
	delete(m.memories, memory.ID)
	return nil
}
