package toolinstall

import (
	"testing"

	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

func TestIsNpmRef(t *testing.T) {
	assert.True(t, isNpmRef("npm:@scope/pkg"))
	assert.True(t, isNpmRef("npm:pkg"))
	assert.False(t, isNpmRef("owner/repo@v1"))
	assert.False(t, isNpmRef(""))
	assert.False(t, isNpmRef("false"))
}

func TestParseNpmRef(t *testing.T) {
	tests := []struct {
		ref     string
		pkg     string
		version string
	}{
		{"npm:@googleworkspace/cli", "@googleworkspace/cli", ""},
		{"npm:@googleworkspace/cli@1.0.0", "@googleworkspace/cli", "1.0.0"},
		{"npm:typescript", "typescript", ""},
		{"npm:typescript@5.0.0", "typescript", "5.0.0"},
		{"npm:@scope/pkg@latest", "@scope/pkg", "latest"},
	}

	for _, tt := range tests {
		t.Run(tt.ref, func(t *testing.T) {
			pkg, version, err := parseNpmRef(tt.ref)
			require.NoError(t, err)
			assert.Equal(t, tt.pkg, pkg)
			assert.Equal(t, tt.version, version)
		})
	}
}

func TestParseNpmRef_Invalid(t *testing.T) {
	invalid := []struct {
		ref  string
		desc string
	}{
		{"npm:", "empty after prefix"},
		{"npm:  ", "whitespace only"},
		{"npm:@", "bare @"},
		{"npm:@scope", "scope without package name"},
		{"npm:@/pkg", "empty scope"},
		{"npm:@scope/", "empty name after scope"},
		{"npm:@scope/@pkg", "@ in package name"},
	}

	for _, tt := range invalid {
		t.Run(tt.desc, func(t *testing.T) {
			_, _, err := parseNpmRef(tt.ref)
			assert.Error(t, err, "parseNpmRef(%q) should return an error", tt.ref)
		})
	}
}

func TestEnsureCommand_NpmRef_DisabledGlobally(t *testing.T) {
	t.Setenv("DOCKER_AGENT_AUTO_INSTALL", "false")
	result, err := EnsureCommand(t.Context(), "gws", "npm:@googleworkspace/cli")
	require.NoError(t, err)
	assert.Equal(t, "gws", result)
}

func TestEnsureCommand_NpmRef_DisabledPerToolset(t *testing.T) {
	for _, value := range []string{"false", "off", "False", "OFF"} {
		result, err := EnsureCommand(t.Context(), "gws", value)
		require.NoError(t, err)
		assert.Equal(t, "gws", result)
	}
}
