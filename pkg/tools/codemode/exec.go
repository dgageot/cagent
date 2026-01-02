package codemode

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"slices"

	"github.com/dop251/goja"

	"github.com/docker/cagent/pkg/tools"
)

type ScriptResult struct {
	Value     string         `json:"value" jsonschema:"The value returned by the script"`
	StdOut    string         `json:"stdout" jsonschema:"The standard output of the console"`
	StdErr    string         `json:"stderr" jsonschema:"The standard error of the console"`
	ToolCalls []ToolCallInfo `json:"tool_calls,omitempty" jsonschema:"The list of tool calls made during script execution, only included on failure"`
}

// ToolCallInfo contains information about a tool call made during script execution.
type ToolCallInfo struct {
	Name      string `json:"name" jsonschema:"The name of the tool that was called"`
	Arguments any    `json:"arguments" jsonschema:"The arguments passed to the tool"`
	Result    string `json:"result,omitempty" jsonschema:"The raw response returned by the tool"`
	Error     string `json:"error,omitempty" jsonschema:"The error message, if the tool call failed"`
}

// toolCallTracker tracks tool calls made during script execution.
type toolCallTracker struct {
	calls []ToolCallInfo
}

func (t *toolCallTracker) record(info ToolCallInfo) {
	t.calls = append(t.calls, info)
}

func (c *codeModeTool) runJavascript(ctx context.Context, script string) (ScriptResult, error) {
	vm := goja.New()
	tracker := &toolCallTracker{}

	// Inject console object to the help the LLM debug its own code.
	var (
		stdOut bytes.Buffer
		stdErr bytes.Buffer
	)
	_ = vm.Set("console", console(&stdOut, &stdErr))

	// Inject every tool as a javascript function.
	for _, toolset := range c.toolsets {
		allTools, err := toolset.Tools(ctx)
		if err != nil {
			return ScriptResult{}, err
		}

		for _, tool := range allTools {
			_ = vm.Set(tool.Name, callTool(ctx, tool, tracker))
		}
	}

	// Wrap the user script in an IIFE to allow top-level returns.
	script = "(() => {\n" + script + "\n})()"

	// Run the script.
	v, err := vm.RunString(script)
	if err != nil {
		// Script execution failed - include tool call history to help LLM understand what went wrong
		return ScriptResult{
			StdOut:    stdOut.String(),
			StdErr:    stdErr.String(),
			Value:     err.Error(),
			ToolCalls: tracker.calls,
		}, nil
	}

	value := ""
	if result := v.Export(); result != nil {
		value = fmt.Sprintf("%v", result)
	}

	// Success case - don't include tool calls to avoid unnecessary overhead
	return ScriptResult{
		StdOut: stdOut.String(),
		StdErr: stdErr.String(),
		Value:  value,
	}, nil
}

func callTool(ctx context.Context, tool tools.Tool, tracker *toolCallTracker) func(args map[string]any) (string, error) {
	return func(args map[string]any) (string, error) {
		var toolArgs struct {
			Required []string `json:"required"`
		}

		if err := tools.ConvertSchema(tool.Parameters, &toolArgs); err != nil {
			tracker.record(ToolCallInfo{
				Name:      tool.Name,
				Arguments: args,
				Error:     err.Error(),
			})
			return "", err
		}

		nonNilArgs := make(map[string]any)
		for k, v := range args {
			if slices.Contains(toolArgs.Required, k) || v != nil {
				nonNilArgs[k] = v
			}
		}

		arguments, err := json.Marshal(nonNilArgs)
		if err != nil {
			tracker.record(ToolCallInfo{
				Name:      tool.Name,
				Arguments: nonNilArgs,
				Error:     err.Error(),
			})
			return "", err
		}

		result, err := tool.Handler(ctx, tools.ToolCall{
			Function: tools.FunctionCall{
				Name:      tool.Name,
				Arguments: string(arguments),
			},
		})
		if err != nil {
			tracker.record(ToolCallInfo{
				Name:      tool.Name,
				Arguments: nonNilArgs,
				Error:     err.Error(),
			})
			return "", err
		}

		// In Code Mode, prefer structured Meta data over plain Output
		output := result.Output
		if result.Meta != nil {
			if jsonOutput, err := json.Marshal(result.Meta); err == nil {
				output = string(jsonOutput)
			}
		}

		tracker.record(ToolCallInfo{
			Name:      tool.Name,
			Arguments: nonNilArgs,
			Result:    output,
		})

		return output, nil
	}
}
