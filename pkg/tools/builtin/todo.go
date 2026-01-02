package builtin

import (
	"context"
	"fmt"
	"strings"
	"sync"

	"github.com/docker/cagent/pkg/concurrent"
	"github.com/docker/cagent/pkg/tools"
)

const (
	ToolNameCreateTodo  = "create_todo"
	ToolNameCreateTodos = "create_todos"
	ToolNameUpdateTodo  = "update_todo"
	ToolNameListTodos   = "list_todos"
)

type TodoTool struct {
	tools.BaseToolSet
	handler *todoHandler
}

// Make sure Todo Tool implements the ToolSet Interface
var _ tools.ToolSet = (*TodoTool)(nil)

type Todo struct {
	ID          string `json:"id" jsonschema:"ID of the todo item"`
	Description string `json:"description" jsonschema:"Description of the todo item"`
	Status      string `json:"status" jsonschema:"New status (pending, in-progress,completed)"`
}

type CreateTodoArgs struct {
	Description string `json:"description" jsonschema:"Description of the todo item"`
}

type CreateTodosArgs struct {
	Descriptions []string `json:"descriptions" jsonschema:"Descriptions of the todo items"`
}

type UpdateTodoArgs struct {
	ID     string `json:"id" jsonschema:"ID of the todo item"`
	Status string `json:"status" jsonschema:"New status (pending, in-progress,completed)"`
}

type todoHandler struct {
	todos *concurrent.Slice[Todo]
}

var NewSharedTodoTool = sync.OnceValue(NewTodoTool)

func NewTodoTool() *TodoTool {
	return &TodoTool{
		handler: &todoHandler{
			todos: concurrent.NewSlice[Todo](),
		},
	}
}

func (t *TodoTool) Instructions() string {
	return `## Using the Todo Tools

IMPORTANT: You MUST use these tools to track the progress of your tasks:

1. Before starting any complex task:
	- Create a todo for each major step using create_todo
	- Break down complex steps into smaller todos

2. While working:
	- Use list_todos frequently to keep track of remaining work
	- Mark todos as "completed" when finished

3. Task Management Rules:
	- Never start a new task without creating a todo for it
	- Always check list_todos before responding to ensure no steps are missed
	- Update todo status to reflect current progress

This toolset is REQUIRED for maintaining task state and ensuring all steps are completed.`
}

func (h *todoHandler) createTodo(_ context.Context, params CreateTodoArgs) (*tools.ToolCallResult, error) {
	id := fmt.Sprintf("todo_%d", h.todos.Length()+1)
	todo := Todo{
		ID:          id,
		Description: params.Description,
		Status:      "pending",
	}
	h.todos.Append(todo)

	return &tools.ToolCallResult{
		Output: fmt.Sprintf("Created todo [%s]: %s", id, params.Description),
		Meta:   h.todos.All(),
	}, nil
}

func (h *todoHandler) createTodos(_ context.Context, params CreateTodosArgs) (*tools.ToolCallResult, error) {
	start := h.todos.Length()
	for i, desc := range params.Descriptions {
		id := fmt.Sprintf("todo_%d", start+i+1)
		h.todos.Append(Todo{
			ID:          id,
			Description: desc,
			Status:      "pending",
		})
	}

	var output strings.Builder
	fmt.Fprintf(&output, "Created %d todos: ", len(params.Descriptions))
	for i := range params.Descriptions {
		if i > 0 {
			output.WriteString(", ")
		}
		fmt.Fprintf(&output, "[todo_%d]", start+i+1)
	}

	return &tools.ToolCallResult{
		Output: output.String(),
		Meta:   h.todos.All(),
	}, nil
}

func (h *todoHandler) updateTodo(_ context.Context, params UpdateTodoArgs) (*tools.ToolCallResult, error) {
	_, idx := h.todos.Find(func(t Todo) bool { return t.ID == params.ID })
	if idx == -1 {
		return tools.ResultError(fmt.Sprintf("todo [%s] not found", params.ID)), nil
	}

	h.todos.Update(idx, func(t Todo) Todo {
		t.Status = params.Status
		return t
	})

	return &tools.ToolCallResult{
		Output: fmt.Sprintf("Updated todo [%s] to status: [%s]", params.ID, params.Status),
		Meta:   h.todos.All(),
	}, nil
}

func (h *todoHandler) listTodos(_ context.Context, _ map[string]any) (*tools.ToolCallResult, error) {
	allTodos := h.todos.All()

	var output strings.Builder
	output.WriteString("Current todos:\n")

	for _, todo := range allTodos {
		fmt.Fprintf(&output, "- [%s] %s (Status: %s)\n", todo.ID, todo.Description, todo.Status)
	}

	return &tools.ToolCallResult{
		Output: output.String(),
		Meta:   allTodos,
	}, nil
}

func (t *TodoTool) Tools(context.Context) ([]tools.Tool, error) {
	return []tools.Tool{
		{
			Name:                 ToolNameCreateTodo,
			Category:             "todo",
			Description:          "Create a new todo item with a description",
			Parameters:           tools.MustSchemaFor[CreateTodoArgs](),
			OutputSchema:         tools.MustSchemaFor[string](),
			CodeModeOutputSchema: tools.MustSchemaFor[[]Todo](),
			Handler:              NewHandler(t.handler.createTodo),
			Annotations: tools.ToolAnnotations{
				Title:        "Create TODO",
				ReadOnlyHint: true, // Technically not read-only but has practically no destructive side effects.
			},
		},
		{
			Name:                 ToolNameCreateTodos,
			Category:             "todo",
			Description:          "Create a list of new todo items with descriptions",
			Parameters:           tools.MustSchemaFor[CreateTodosArgs](),
			OutputSchema:         tools.MustSchemaFor[string](),
			CodeModeOutputSchema: tools.MustSchemaFor[[]Todo](),
			Handler:              NewHandler(t.handler.createTodos),
			Annotations: tools.ToolAnnotations{
				Title:        "Create TODOs",
				ReadOnlyHint: true, // Technically not read-only but has practically no destructive side effects.
			},
		},
		{
			Name:                 ToolNameUpdateTodo,
			Category:             "todo",
			Description:          "Update the status of a todo item",
			Parameters:           tools.MustSchemaFor[UpdateTodoArgs](),
			OutputSchema:         tools.MustSchemaFor[string](),
			CodeModeOutputSchema: tools.MustSchemaFor[[]Todo](),
			Handler:              NewHandler(t.handler.updateTodo),
			Annotations: tools.ToolAnnotations{
				Title:        "Update TODO",
				ReadOnlyHint: true, // Technically not read-only but has practically no destructive side effects.
			},
		},
		{
			Name:                 ToolNameListTodos,
			Category:             "todo",
			Description:          "List all current todos with their status",
			OutputSchema:         tools.MustSchemaFor[string](),
			CodeModeOutputSchema: tools.MustSchemaFor[[]Todo](),
			Handler:              NewHandler(t.handler.listTodos),
			Annotations: tools.ToolAnnotations{
				Title:        "List TODOs",
				ReadOnlyHint: true,
			},
		},
	}, nil
}
