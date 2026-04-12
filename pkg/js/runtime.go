package js

import (
	"context"
	"encoding/json"
	"fmt"
	"log/slog"
	"slices"
	"strings"

	"github.com/dop251/goja"

	"github.com/docker/docker-agent/pkg/config/types"
	"github.com/docker/docker-agent/pkg/environment"
	"github.com/docker/docker-agent/pkg/tools"
)

// newVM creates a new Goja JavaScript runtime.
var newVM = goja.New

// Runtime expands JavaScript template literals with reusable bindings.
type Runtime struct {
	env   environment.Provider
	tools []tools.Tool
}

// New creates a reusable JavaScript runtime helper.
func New(env environment.Provider, agentTools []tools.Tool) *Runtime {
	return &Runtime{
		env:   env,
		tools: agentTools,
	}
}

// dynamicLookup implements goja.DynamicObject for lazy key-value access.
type dynamicLookup struct {
	vm     *goja.Runtime
	lookup func(string) string
}

func (d *dynamicLookup) Get(k string) goja.Value   { return d.vm.ToValue(d.lookup(k)) }
func (*dynamicLookup) Set(string, goja.Value) bool { return false }
func (*dynamicLookup) Has(string) bool             { return true }
func (*dynamicLookup) Delete(string) bool          { return true }
func (*dynamicLookup) Keys() []string              { return nil }

// Evaluate finds and evaluates ${...} JavaScript expressions in the input string.
// args are available as the 'args' array in JavaScript.
func (r *Runtime) Evaluate(ctx context.Context, input string, args []string) string {
	if !strings.Contains(input, "${") {
		return input
	}

	vm := r.newVM(ctx)
	if args == nil {
		args = []string{}
	}
	_ = vm.Set("args", args)

	slog.Debug("Evaluating JS template", "input", input)

	return runExpansion(vm, input)
}

// Expand expands JavaScript template literals using the provided values map.
// The values are bound as top-level variables in the JS runtime alongside the
// env object from the runtime's environment provider.
func (r *Runtime) Expand(ctx context.Context, text string, values map[string]string) string {
	if !strings.Contains(text, "${") {
		return text
	}

	vm := r.newVM(ctx)
	for k, v := range values {
		_ = vm.Set(k, v)
	}

	return runExpansion(vm, text)
}

// ExpandMap expands JavaScript template literals in all values of the given map.
func (r *Runtime) ExpandMap(ctx context.Context, kv map[string]string) map[string]string {
	if kv == nil {
		return nil
	}

	vm := r.newVM(ctx)

	expanded := make(map[string]string, len(kv))
	for k, v := range kv {
		expanded[k] = runExpansion(vm, v)
	}
	return expanded
}

// ExpandCommands expands JavaScript template literals in all command fields.
func (r *Runtime) ExpandCommands(ctx context.Context, cmds types.Commands) types.Commands {
	if cmds == nil {
		return nil
	}

	vm := r.newVM(ctx)

	expanded := make(types.Commands, len(cmds))
	for k, cmd := range cmds {
		expanded[k] = types.Command{
			Description: runExpansion(vm, cmd.Description),
			Instruction: runExpansion(vm, cmd.Instruction),
		}
	}
	return expanded
}

// ExpandWithLookup expands JavaScript template literals in map values.
// It binds a dynamic object with the given name to the JS runtime,
// using lookup to resolve property accesses. Each value is optionally
// preprocessed with preprocess before expansion (pass nil to skip).
func ExpandWithLookup(values map[string]string, objName string, lookup, preprocess func(string) string) map[string]string {
	vm := newVM()
	_ = vm.Set(objName, vm.NewDynamicObject(&dynamicLookup{
		vm:     vm,
		lookup: lookup,
	}))

	resolved := make(map[string]string, len(values))
	for k, v := range values {
		if preprocess != nil {
			v = preprocess(v)
		}
		resolved[k] = runExpansion(vm, v)
	}
	return resolved
}

// newVM creates a new JS runtime with all reusable bindings pre-bound.
func (r *Runtime) newVM(ctx context.Context) *goja.Runtime {
	vm := newVM()

	if r.env != nil {
		_ = vm.Set("env", vm.NewDynamicObject(&dynamicLookup{
			vm:     vm,
			lookup: func(k string) string { v, _ := r.env.Get(ctx, k); return v },
		}))
	}

	for _, tool := range r.tools {
		_ = vm.Set(tool.Name, r.createToolCaller(ctx, tool))
	}

	return vm
}

// createToolCaller creates a JavaScript function that calls the given tool.
func (r *Runtime) createToolCaller(ctx context.Context, tool tools.Tool) func(args map[string]any) (string, error) {
	return func(args map[string]any) (string, error) {
		var toolArgs struct {
			Required []string `json:"required"`
		}

		if err := tools.ConvertSchema(tool.Parameters, &toolArgs); err != nil {
			return "", err
		}

		// Filter out nil values for non-required arguments
		nonNilArgs := make(map[string]any)
		for k, v := range args {
			if slices.Contains(toolArgs.Required, k) || v != nil {
				nonNilArgs[k] = v
			}
		}

		arguments, err := json.Marshal(nonNilArgs)
		if err != nil {
			return "", err
		}

		toolCall := tools.ToolCall{
			ID:   "jseval_" + tool.Name,
			Type: "function",
			Function: tools.FunctionCall{
				Name:      tool.Name,
				Arguments: string(arguments),
			},
		}

		if tool.Handler == nil {
			return "", fmt.Errorf("tool '%s' has no handler", tool.Name)
		}

		result, err := tool.Handler(ctx, toolCall)
		if err != nil {
			return "", err
		}

		return result.Output, nil
	}
}

// runExpansion executes the template string using the provided Goja runtime.
func runExpansion(vm *goja.Runtime, text string) string {
	// Escape backslashes first, then backticks
	escaped := strings.ReplaceAll(text, "\\", "\\\\")
	escaped = strings.ReplaceAll(escaped, "`", "\\`")
	script := "`" + escaped + "`"

	v, err := vm.RunString(script)
	if err != nil {
		return text
	}

	if v == nil || v.Export() == nil {
		return ""
	}

	return v.String()
}
