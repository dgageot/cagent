package js

import "github.com/docker/docker-agent/pkg/tools"

// Deprecated: use New instead.
func NewEvaluator(agentTools []tools.Tool) *Runtime {
	return New(nil, agentTools)
}
