package latest

import (
	"testing"

	"github.com/stretchr/testify/assert"
)

func TestModelConfigUnloadOnSwitch(t *testing.T) {
	t.Parallel()

	tests := []struct {
		name string
		cfg  *ModelConfig
		want bool
	}{
		{name: "nil config", cfg: nil, want: false},
		{name: "no provider opts", cfg: &ModelConfig{}, want: false},
		{name: "key absent", cfg: &ModelConfig{ProviderOpts: map[string]any{"other": true}}},
		{name: "explicit false", cfg: &ModelConfig{ProviderOpts: map[string]any{"unload_on_switch": false}}},
		{name: "explicit true", cfg: &ModelConfig{ProviderOpts: map[string]any{"unload_on_switch": true}}, want: true},
		{name: "non-bool ignored", cfg: &ModelConfig{ProviderOpts: map[string]any{"unload_on_switch": "true"}}, want: false},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			t.Parallel()
			assert.Equal(t, tt.want, tt.cfg.UnloadOnSwitch())
		})
	}
}

func TestModelConfigUnloadAPI(t *testing.T) {
	t.Parallel()

	tests := []struct {
		name string
		cfg  *ModelConfig
		want string
	}{
		{name: "nil config", cfg: nil, want: ""},
		{name: "no provider opts", cfg: &ModelConfig{}, want: ""},
		{name: "key absent", cfg: &ModelConfig{ProviderOpts: map[string]any{"other": "/foo"}}},
		{name: "valid path", cfg: &ModelConfig{ProviderOpts: map[string]any{"unload_api": "/api/unload"}}, want: "/api/unload"},
		{name: "trims whitespace", cfg: &ModelConfig{ProviderOpts: map[string]any{"unload_api": "  /api/unload  "}}, want: "/api/unload"},
		{name: "non-string ignored", cfg: &ModelConfig{ProviderOpts: map[string]any{"unload_api": 42}}, want: ""},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			t.Parallel()
			assert.Equal(t, tt.want, tt.cfg.UnloadAPI())
		})
	}
}
