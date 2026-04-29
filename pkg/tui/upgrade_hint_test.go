package tui

import (
	"strings"
	"testing"

	"gotest.tools/v3/assert"

	"github.com/docker/docker-agent/pkg/version/check"
)

func TestBuildStatusBarTitle(t *testing.T) {
	t.Run("no upgrade", func(t *testing.T) {
		check.SeedCacheForTest(t, "v1.0.0")
		assert.Equal(t, "docker agent v1.0.0", buildStatusBarTitle("docker agent", "v1.0.0"))
	})

	t.Run("upgrade available", func(t *testing.T) {
		check.SeedCacheForTest(t, "v1.2.3")
		got := buildStatusBarTitle("docker agent", "v1.0.0")
		assert.Assert(t, strings.Contains(got, "docker agent v1.0.0"))
		assert.Assert(t, strings.Contains(got, "update available: v1.2.3"))
	})

	t.Run("dev build is silent", func(t *testing.T) {
		check.SeedCacheForTest(t, "v1.2.3")
		assert.Equal(t, "docker agent dev", buildStatusBarTitle("docker agent", "dev"))
	})

	t.Run("disabled is silent", func(t *testing.T) {
		check.SeedCacheForTest(t, "v1.2.3")
		t.Setenv(check.DisableEnvVar, "1")
		assert.Equal(t, "docker agent v1.0.0", buildStatusBarTitle("docker agent", "v1.0.0"))
	})
}
