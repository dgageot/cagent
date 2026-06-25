package teamloader

import (
	"context"

	"github.com/docker/docker-agent/pkg/tools"
)

// WithReadOnlyFilter wraps a toolset so it only lists and exposes tools whose
// annotations carry a read-only hint. Every other tool is filtered out, so the
// agent can never call a mutating tool from this toolset. When readOnly is
// false the inner toolset is returned unchanged.
func WithReadOnlyFilter(inner tools.ToolSet, readOnly bool) tools.ToolSet {
	if !readOnly {
		return inner
	}

	return &readOnlyTools{ToolSet: inner}
}

type readOnlyTools struct {
	tools.ToolSet
}

// Verify interface compliance
var (
	_ tools.Instructable = (*readOnlyTools)(nil)
	_ tools.Unwrapper    = (*readOnlyTools)(nil)
)

// Unwrap implements tools.Unwrapper.
func (f *readOnlyTools) Unwrap() tools.ToolSet {
	return f.ToolSet
}

// Instructions implements tools.Instructable by delegating to the inner toolset.
func (f *readOnlyTools) Instructions() string {
	return tools.GetInstructions(f.ToolSet)
}

func (f *readOnlyTools) Tools(ctx context.Context) ([]tools.Tool, error) {
	allTools, err := f.ToolSet.Tools(ctx)
	if err != nil {
		return nil, err
	}

	return tools.FilterReadOnly(allTools), nil
}
