package teamloader

import (
	"context"
	"testing"

	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"

	"github.com/docker/docker-agent/pkg/tools"
)

func readOnlyTool(name string, readOnly bool) tools.Tool {
	return tools.Tool{
		Name:        name,
		Annotations: tools.ToolAnnotations{ReadOnlyHint: readOnly},
	}
}

func TestWithReadOnlyFilter_Disabled(t *testing.T) {
	inner := &mockToolSet{}

	wrapped := WithReadOnlyFilter(inner, false)

	assert.Same(t, inner, wrapped)
}

func TestWithReadOnlyFilter_KeepsOnlyReadOnly(t *testing.T) {
	inner := &mockToolSet{
		toolsFunc: func(context.Context) ([]tools.Tool, error) {
			return []tools.Tool{
				readOnlyTool("read_file", true),
				readOnlyTool("write_file", false),
				readOnlyTool("list_files", true),
			}, nil
		},
	}

	wrapped := WithReadOnlyFilter(inner, true)

	result, err := wrapped.Tools(t.Context())
	require.NoError(t, err)
	require.Len(t, result, 2)
	assert.Equal(t, "read_file", result[0].Name)
	assert.Equal(t, "list_files", result[1].Name)
}

func TestWithReadOnlyFilter_NoReadOnlyTools(t *testing.T) {
	inner := &mockToolSet{
		toolsFunc: func(context.Context) ([]tools.Tool, error) {
			return []tools.Tool{
				readOnlyTool("write_file", false),
				readOnlyTool("shell", false),
			}, nil
		},
	}

	wrapped := WithReadOnlyFilter(inner, true)

	result, err := wrapped.Tools(t.Context())
	require.NoError(t, err)
	assert.Empty(t, result)
}

func TestWithReadOnlyFilter_InstructablePassthrough(t *testing.T) {
	inner := &instructableToolSet{
		mockToolSet: mockToolSet{
			toolsFunc: func(context.Context) ([]tools.Tool, error) {
				return []tools.Tool{readOnlyTool("read_file", true)}, nil
			},
		},
		instructions: "Read-only instructions",
	}

	wrapped := WithReadOnlyFilter(inner, true)

	assert.Equal(t, "Read-only instructions", tools.GetInstructions(wrapped))
}
