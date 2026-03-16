package builtin

import (
	"context"
	"encoding/json"
	"fmt"
	"strings"
	"sync"
	"sync/atomic"

	"github.com/docker/docker-agent/pkg/concurrent"
	"github.com/docker/docker-agent/pkg/tools"
)

const (
	ToolNameCreateTodo  = "create_todo"
	ToolNameCreateTodos = "create_todos"
	ToolNameUpdateTodos = "update_todos"
	ToolNameListTodos   = "list_todos"
)

type TodoTool struct {
	handler *todoHandler
}

// Verify interface compliance
var (
	_ tools.ToolSet      = (*TodoTool)(nil)
	_ tools.Instructable = (*TodoTool)(nil)
)

type Todo struct {
	ID          string `json:"id" jsonschema:"ID of the todo item"`
	Description string `json:"description" jsonschema:"Description of the todo item"`
	Status      string `json:"status" jsonschema:"Status of the todo item (pending, in-progress, completed)"`
}

type CreateTodoArgs struct {
	Description string `json:"description" jsonschema:"Description of the todo item"`
}

type CreateTodosArgs struct {
	Descriptions []string `json:"descriptions" jsonschema:"Descriptions of the todo items"`
}

type TodoUpdate struct {
	ID     string `json:"id" jsonschema:"ID of the todo item"`
	Status string `json:"status" jsonschema:"New status (pending, in-progress, completed)"`
}

type UpdateTodosArgs struct {
	Updates []TodoUpdate `json:"updates" jsonschema:"List of todo updates"`
}

// Output types for JSON-structured responses.

type CreateTodoOutput struct {
	Created  Todo   `json:"created" jsonschema:"The created todo item"`
	AllTodos []Todo `json:"all_todos" jsonschema:"Current state of all todo items"`
	Reminder string `json:"reminder,omitempty" jsonschema:"Reminder about incomplete todos that still need to be completed"`
}

type CreateTodosOutput struct {
	Created  []Todo `json:"created" jsonschema:"List of created todo items"`
	AllTodos []Todo `json:"all_todos" jsonschema:"Current state of all todo items"`
	Reminder string `json:"reminder,omitempty" jsonschema:"Reminder about incomplete todos that still need to be completed"`
}

type UpdateTodosOutput struct {
	Updated  []TodoUpdate `json:"updated,omitempty" jsonschema:"List of successfully updated todos"`
	NotFound []string     `json:"not_found,omitempty" jsonschema:"IDs of todos that were not found"`
	AllTodos []Todo       `json:"all_todos" jsonschema:"Current state of all todo items"`
	Reminder string       `json:"reminder,omitempty" jsonschema:"Reminder about incomplete todos that still need to be completed"`
}

type ListTodosOutput struct {
	Todos    []Todo `json:"todos" jsonschema:"List of all current todo items"`
	Reminder string `json:"reminder,omitempty" jsonschema:"Reminder about incomplete todos that still need to be completed"`
}

// TodoStorage defines the storage layer for todo items.
type TodoStorage interface {
	// Add appends a new todo item.
	Add(todo Todo)
	// All returns a copy of all todo items.
	All() []Todo
	// Len returns the number of todo items.
	Len() int
	// NextID returns a unique, monotonically increasing ID for a new todo.
	NextID() int64
	// UpdateByID atomically finds a todo by ID and applies fn to it.
	// It returns true if the todo was found and updated, false otherwise.
	UpdateByID(id string, fn func(Todo) Todo) bool
	// Clear removes all todo items.
	Clear()
}

// MemoryTodoStorage is an in-memory, concurrency-safe implementation of TodoStorage.
type MemoryTodoStorage struct {
	todos  *concurrent.Slice[Todo]
	nextID atomic.Int64
}

func NewMemoryTodoStorage() *MemoryTodoStorage {
	return &MemoryTodoStorage{
		todos: concurrent.NewSlice[Todo](),
	}
}

func (s *MemoryTodoStorage) Add(todo Todo) {
	s.todos.Append(todo)
}

func (s *MemoryTodoStorage) All() []Todo {
	all := s.todos.All()
	if all == nil {
		return []Todo{}
	}
	return all
}

func (s *MemoryTodoStorage) Len() int {
	return s.todos.Length()
}

func (s *MemoryTodoStorage) NextID() int64 {
	return s.nextID.Add(1)
}

func (s *MemoryTodoStorage) UpdateByID(id string, fn func(Todo) Todo) bool {
	return s.todos.FindAndUpdate(func(t Todo) bool { return t.ID == id }, fn)
}

func (s *MemoryTodoStorage) Clear() {
	s.todos.Clear()
}

// TodoOption is a functional option for configuring a TodoTool.
type TodoOption func(*TodoTool)

// WithStorage sets a custom storage implementation for the TodoTool.
// The provided storage must not be nil.
func WithStorage(storage TodoStorage) TodoOption {
	if storage == nil {
		panic("todo: storage must not be nil")
	}
	return func(t *TodoTool) {
		t.handler.storage = storage
	}
}

type todoHandler struct {
	storage TodoStorage
}

var NewSharedTodoTool = sync.OnceValue(func() *TodoTool { return NewTodoTool() })

func NewTodoTool(opts ...TodoOption) *TodoTool {
	t := &TodoTool{
		handler: &todoHandler{
			storage: NewMemoryTodoStorage(),
		},
	}
	for _, opt := range opts {
		opt(t)
	}
	return t
}

func (t *TodoTool) Instructions() string {
	return `## Todo Tools

Track task progress with todos:
- Create todos for each major step before starting complex work (prefer batch create_todos)
- Update status to "in-progress" before starting, "completed" immediately after finishing
- Every todo MUST be marked "completed" before your final response
- Batch multiple updates in a single update_todos call
- Never leave todos pending or in-progress when done`
}

// addTodo creates a new todo and adds it to storage.
func (h *todoHandler) addTodo(description string) Todo {
	todo := Todo{
		ID:          fmt.Sprintf("todo_%d", h.storage.NextID()),
		Description: description,
		Status:      "pending",
	}
	h.storage.Add(todo)
	return todo
}

// jsonResult builds a ToolCallResult with a JSON-serialized output and allTodos as Meta.
func (h *todoHandler) jsonResult(v any, allTodos []Todo) (*tools.ToolCallResult, error) {
	out, err := json.Marshal(v)
	if err != nil {
		return nil, fmt.Errorf("marshaling todo output: %w", err)
	}
	return &tools.ToolCallResult{
		Output: string(out),
		Meta:   allTodos,
	}, nil
}

func (h *todoHandler) createTodo(_ context.Context, params CreateTodoArgs) (*tools.ToolCallResult, error) {
	created := h.addTodo(params.Description)
	allTodos := h.storage.All()
	return h.jsonResult(CreateTodoOutput{
		Created:  created,
		AllTodos: allTodos,
		Reminder: incompleteReminder(allTodos),
	}, allTodos)
}

func (h *todoHandler) createTodos(_ context.Context, params CreateTodosArgs) (*tools.ToolCallResult, error) {
	created := make([]Todo, 0, len(params.Descriptions))
	for _, desc := range params.Descriptions {
		created = append(created, h.addTodo(desc))
	}
	allTodos := h.storage.All()
	return h.jsonResult(CreateTodosOutput{
		Created:  created,
		AllTodos: allTodos,
		Reminder: incompleteReminder(allTodos),
	}, allTodos)
}

// validTodoStatuses defines the set of allowed todo statuses.
var validTodoStatuses = map[string]bool{
	"pending":     true,
	"in-progress": true,
	"completed":   true,
}

func (h *todoHandler) updateTodos(_ context.Context, params UpdateTodosArgs) (*tools.ToolCallResult, error) {
	for _, update := range params.Updates {
		if !validTodoStatuses[update.Status] {
			errMsg := fmt.Sprintf("invalid status %q for todo %s: must be one of pending, in-progress, completed", update.Status, update.ID)
			out, err := json.Marshal(map[string]string{"error": errMsg})
			if err != nil {
				return nil, fmt.Errorf("marshaling todo error: %w", err)
			}
			return &tools.ToolCallResult{
				Output:  string(out),
				IsError: true,
				Meta:    h.storage.All(),
			}, nil
		}
	}

	result := UpdateTodosOutput{}

	for _, update := range params.Updates {
		ok := h.storage.UpdateByID(update.ID, func(t Todo) Todo {
			t.Status = update.Status
			return t
		})
		if !ok {
			result.NotFound = append(result.NotFound, update.ID)
			continue
		}
		result.Updated = append(result.Updated, update)
	}

	allTodos := h.storage.All()
	result.AllTodos = allTodos
	result.Reminder = incompleteReminder(allTodos)

	if len(result.NotFound) > 0 && len(result.Updated) == 0 {
		res, err := h.jsonResult(result, allTodos)
		if err != nil {
			return nil, err
		}
		res.IsError = true
		return res, nil
	}

	return h.jsonResult(result, allTodos)
}

// incompleteReminder returns a reminder string listing any non-completed todos,
// or an empty string if all are completed (or the list is empty).
func incompleteReminder(all []Todo) string {
	var b strings.Builder
	for _, todo := range all {
		var prefix string
		switch todo.Status {
		case "pending":
			prefix = " (pending) "
		case "in-progress":
			prefix = " (in-progress) "
		default:
			continue
		}
		if b.Len() == 0 {
			b.WriteString("The following todos are still incomplete and MUST be completed:")
		}
		b.WriteString(prefix)
		fmt.Fprintf(&b, "[%s] %s", todo.ID, todo.Description)
	}
	return b.String()
}

func (h *todoHandler) listTodos(_ context.Context, _ tools.ToolCall) (*tools.ToolCallResult, error) {
	todos := h.storage.All()
	out := ListTodosOutput{Todos: todos}
	out.Reminder = incompleteReminder(todos)
	return h.jsonResult(out, todos)
}

func (t *TodoTool) Tools(context.Context) ([]tools.Tool, error) {
	return []tools.Tool{
		{
			Name:         ToolNameCreateTodo,
			Category:     "todo",
			Description:  "Create a new todo item with a description",
			Parameters:   tools.MustSchemaFor[CreateTodoArgs](),
			OutputSchema: tools.MustSchemaFor[CreateTodoOutput](),
			Handler:      tools.NewHandler(t.handler.createTodo),
			Annotations: tools.ToolAnnotations{
				Title:        "Create TODO",
				ReadOnlyHint: true, // Technically not read-only but has practically no destructive side effects.
			},
		},
		{
			Name:         ToolNameCreateTodos,
			Category:     "todo",
			Description:  "Create a list of new todo items with descriptions",
			Parameters:   tools.MustSchemaFor[CreateTodosArgs](),
			OutputSchema: tools.MustSchemaFor[CreateTodosOutput](),
			Handler:      tools.NewHandler(t.handler.createTodos),
			Annotations: tools.ToolAnnotations{
				Title:        "Create TODOs",
				ReadOnlyHint: true, // Technically not read-only but has practically no destructive side effects.
			},
		},
		{
			Name:         ToolNameUpdateTodos,
			Category:     "todo",
			Description:  "Update the status of one or more todo item(s)",
			Parameters:   tools.MustSchemaFor[UpdateTodosArgs](),
			OutputSchema: tools.MustSchemaFor[UpdateTodosOutput](),
			Handler:      tools.NewHandler(t.handler.updateTodos),
			Annotations: tools.ToolAnnotations{
				Title:        "Update TODOs",
				ReadOnlyHint: true, // Technically not read-only but has practically no destructive side effects.
			},
		},
		{
			Name:         ToolNameListTodos,
			Category:     "todo",
			Description:  "List all current todos with their status",
			OutputSchema: tools.MustSchemaFor[ListTodosOutput](),
			Handler:      t.handler.listTodos,
			Annotations: tools.ToolAnnotations{
				Title:        "List TODOs",
				ReadOnlyHint: true,
			},
		},
	}, nil
}
