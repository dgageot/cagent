package tools

import (
	"encoding/json"
)

const (
	// DescriptionParam is the parameter name for the description
	DescriptionParam = "description"
)

// AddDescriptionParameter adds a "description" parameter to tools that have
// AddDescriptionParameter set to true. This allows the LLM to provide context
// about what it's doing with each tool call.
func AddDescriptionParameter(toolList []Tool) []Tool {
	result := make([]Tool, len(toolList))
	for i, tool := range toolList {
		result[i] = addDescriptionParam(tool)
	}
	return result
}

func addDescriptionParam(tool Tool) Tool {
	if !tool.AddDescriptionParameter {
		return tool
	}

	schema, err := SchemaToMap(tool.Parameters)
	if err != nil {
		return tool
	}

	properties, ok := schema["properties"].(map[string]any)
	if !ok {
		properties = make(map[string]any)
		schema["properties"] = properties
	}

	properties[DescriptionParam] = map[string]any{
		"type":        "string",
		"description": "Brief description of this call",
	}

	tool.Parameters = schema
	return tool
}

// FilterReadOnly returns only the tools whose annotations carry a read-only
// hint. It is used to enforce read-only toolsets and read-only agents: every
// mutating tool is dropped so it can neither be listed nor called.
func FilterReadOnly(toolList []Tool) []Tool {
	var filtered []Tool
	for _, tool := range toolList {
		if tool.Annotations.ReadOnlyHint {
			filtered = append(filtered, tool)
		}
	}
	return filtered
}

// ExtractDescription extracts the description from tool call arguments.
func ExtractDescription(arguments string) string {
	var args map[string]any
	if err := json.Unmarshal([]byte(arguments), &args); err != nil {
		return ""
	}

	if desc, ok := args[DescriptionParam].(string); ok {
		return desc
	}
	return ""
}
