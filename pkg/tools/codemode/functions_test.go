package codemode

import (
	"testing"

	"github.com/stretchr/testify/assert"

	"github.com/docker/cagent/pkg/tools"
)

func TestToolToJsDoc_FallsBackToOutputSchemaWhenCodeModeIsNil(t *testing.T) {
	type Args struct {
		Name string `json:"name"`
	}

	tool := tools.Tool{
		Name:                 "test_tool",
		Description:          "A test tool",
		Parameters:           tools.MustSchemaFor[Args](),
		OutputSchema:         tools.MustSchemaFor[string](),
		CodeModeOutputSchema: nil, // explicitly nil
	}

	jsDoc := toolToJsDoc(tool)

	// Should fall back to OutputSchema which is string
	assert.Contains(t, jsDoc, `And Output follows the following JSON schema:`)
	assert.Contains(t, jsDoc, `"type": "string"`)
}

func TestToolToJsDoc_PrefersCodeModeOutputSchema(t *testing.T) {
	type CreateTodoArgs struct {
		Description string `json:"description" jsonschema:"Description of the todo item"`
	}

	type RichOutput struct {
		ID          string `json:"id" jsonschema:"The unique ID of the created todo"`
		Description string `json:"description" jsonschema:"The description of the todo"`
	}

	tool := tools.Tool{
		Name:                 "create_todo",
		Description:          "Create new todo",
		Parameters:           tools.MustSchemaFor[CreateTodoArgs](),
		OutputSchema:         tools.MustSchemaFor[string](),
		CodeModeOutputSchema: tools.MustSchemaFor[RichOutput](),
	}

	jsDoc := toolToJsDoc(tool)

	// Should use CodeModeOutputSchema (RichOutput) instead of OutputSchema (string)
	assert.Contains(t, jsDoc, `"id":`)
	assert.Contains(t, jsDoc, `"The unique ID of the created todo"`)
	// The output schema is an object, not just a string
	assert.Contains(t, jsDoc, `"additionalProperties": false`)
}

func TestToolToJsDoc(t *testing.T) {
	type CreateTodoArgs struct {
		Description string `json:"description" jsonschema:"Description of the todo item"`
	}

	tool := tools.Tool{
		Name:         "create_todo",
		Description:  "Create new todo\n each of them with a description",
		Parameters:   tools.MustSchemaFor[CreateTodoArgs](),
		OutputSchema: tools.MustSchemaFor[string](),
	}

	jsDoc := toolToJsDoc(tool)

	assert.Equal(t, `
/**
 * Create new todo
 * each of them with a description
 * 
 * @param args - Input object containing the parameters.
 * @returns Output - The result of the function execution.
 *
 * Where Input follows the following JSON schema:
 * {
 *   "type": "object",
 *   "required": [
 *     "description"
 *   ],
 *   "properties": {
 *     "description": {
 *       "type": "string",
 *       "description": "Description of the todo item"
 *     }
 *   },
 *   "additionalProperties": false
 * }
 *
 * And Output follows the following JSON schema:
 * {
 *   "type": "string"
 * }
 */
function create_todo(args: Input): Output { ... }
`, jsDoc)
}
