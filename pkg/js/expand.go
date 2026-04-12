package js

import "github.com/docker/docker-agent/pkg/environment"

// Deprecated: use New instead.
func NewJsExpander(env environment.Provider) *Runtime {
	return New(env, nil)
}

// Deprecated: use ExpandWithLookup instead.
func ExpandMapFunc(values map[string]string, objName string, lookup, preprocess func(string) string) map[string]string {
	return ExpandWithLookup(values, objName, lookup, preprocess)
}
