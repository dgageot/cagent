package remote

import (
	"io"
	"log"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/google/go-containerregistry/pkg/crane"
	"github.com/google/go-containerregistry/pkg/name"
	"github.com/google/go-containerregistry/pkg/registry"
	v1 "github.com/google/go-containerregistry/pkg/v1"
	"github.com/google/go-containerregistry/pkg/v1/empty"
	"github.com/google/go-containerregistry/pkg/v1/mutate"
	"github.com/google/go-containerregistry/pkg/v1/static"
	"github.com/google/go-containerregistry/pkg/v1/types"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"

	"github.com/docker/docker-agent/pkg/content"
)

func TestPullRegistryNotFound(t *testing.T) {
	t.Parallel()

	// Use a test server that returns 404 for fast failure
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusNotFound)
	}))
	defer server.Close()

	// Extract host:port from server URL (remove http://)
	registryHost := strings.TrimPrefix(server.URL, "http://")

	// Test various image references that should fail with 404
	refs := []string{
		registryHost + "/non-existent:latest",
		registryHost + "/test:latest",
	}

	for _, ref := range refs {
		_, err := Pull(t.Context(), ref, false, crane.Insecure)
		require.Error(t, err, "expected error for ref: %s", ref)
	}
}

func TestPullIntegration(t *testing.T) {
	t.Parallel()

	store, err := content.NewStore(content.WithBaseDir(t.TempDir()))
	require.NoError(t, err)

	testData := []byte("test pull integration")
	layer := static.NewLayer(testData, types.OCIUncompressedLayer)
	img := empty.Image
	img, err = mutate.AppendLayers(img, layer)
	require.NoError(t, err)

	testRef := "pull-test:latest"
	digest, err := store.StoreArtifact(img, testRef)
	require.NoError(t, err)

	t.Cleanup(func() {
		if err := store.DeleteArtifact(digest); err != nil {
			t.Logf("Failed to clean up artifact: %v", err)
		}
	})

	retrievedImg, err := store.GetArtifactImage(testRef)
	require.NoError(t, err)
	assert.NotNil(t, retrievedImg)

	err = Push(t.Context(), "invalid:reference:with:too:many:colons")
	require.Error(t, err)
}

func TestNormalizeReference(t *testing.T) {
	t.Parallel()

	tests := []struct {
		name     string
		ref      string
		expected string
	}{
		{
			name:     "short reference gets normalized",
			ref:      "agentcatalog/review-pr",
			expected: "agentcatalog/review-pr:latest",
		},
		{
			name:     "fully qualified reference gets normalized to same key",
			ref:      "index.docker.io/agentcatalog/review-pr:latest",
			expected: "agentcatalog/review-pr:latest",
		},
		{
			name:     "tagged reference preserves tag",
			ref:      "agentcatalog/review-pr:v1",
			expected: "agentcatalog/review-pr:v1",
		},
		{
			name:     "digest reference preserves digest",
			ref:      "agentcatalog/review-pr@sha256:0000000000000000000000000000000000000000000000000000000000000000",
			expected: "agentcatalog/review-pr@sha256:0000000000000000000000000000000000000000000000000000000000000000",
		},
		{
			name:     "non-docker-hub registry",
			ref:      "ghcr.io/myorg/agent:v2",
			expected: "myorg/agent:v2",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			t.Parallel()
			result, err := NormalizeReference(tt.ref)
			require.NoError(t, err)
			assert.Equal(t, tt.expected, result)
		})
	}
}

func TestNormalizeReference_InvalidReference(t *testing.T) {
	t.Parallel()

	_, err := NormalizeReference(":::invalid")
	require.Error(t, err)
}

func TestIsDigestReference(t *testing.T) {
	t.Parallel()

	tests := []struct {
		name     string
		ref      string
		expected bool
	}{
		{"tag reference", "agentcatalog/review-pr:latest", false},
		{"implicit tag", "agentcatalog/review-pr", false},
		{"digest reference", "agentcatalog/review-pr@sha256:0000000000000000000000000000000000000000000000000000000000000000", true},
		{"fully qualified digest", "index.docker.io/agentcatalog/review-pr@sha256:0000000000000000000000000000000000000000000000000000000000000000", true},
		{"invalid reference", ":::invalid", false},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			t.Parallel()
			assert.Equal(t, tt.expected, IsDigestReference(tt.ref))
		})
	}
}

func TestSeparator(t *testing.T) {
	t.Parallel()

	tests := []struct {
		name     string
		ref      string
		expected string
	}{
		{
			name:     "tag reference uses colon",
			ref:      "docker.io/library/alpine:latest",
			expected: ":",
		},
		{
			name:     "digest reference uses at sign",
			ref:      "docker.io/library/alpine@sha256:0000000000000000000000000000000000000000000000000000000000000000",
			expected: "@",
		},
		{
			name:     "short tag reference uses colon",
			ref:      "alpine:v1.0",
			expected: ":",
		},
		{
			name:     "short digest reference uses at sign",
			ref:      "alpine@sha256:0000000000000000000000000000000000000000000000000000000000000000",
			expected: "@",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			ref, err := name.ParseReference(tt.ref)
			require.NoError(t, err)
			assert.Equal(t, tt.expected, separator(ref))
		})
	}
}

// agentImage returns a small OCI image carrying the cagent annotation,
// or no annotation when annotated is false. Useful for exercising the
// annotation-validation code paths.
func agentImage(t *testing.T, annotated bool, body string) v1.Image {
	t.Helper()
	layer := static.NewLayer([]byte(body), "application/yaml")
	img, err := mutate.AppendLayers(empty.Image, layer)
	require.NoError(t, err)
	img = mutate.MediaType(img, types.OCIManifestSchema1)
	if annotated {
		img = mutate.Annotations(img, map[string]string{
			"io.docker.agent.version": "test",
		}).(v1.Image)
	}
	return img
}

func TestReadCached(t *testing.T) {
	t.Parallel()

	store, err := content.NewStore(content.WithBaseDir(t.TempDir()))
	require.NoError(t, err)

	t.Run("missing entry returns ok=false", func(t *testing.T) {
		_, _, ok := readCached(store, "missing/agent:latest")
		assert.False(t, ok)
	})

	t.Run("unannotated entry is treated as cache miss", func(t *testing.T) {
		ref := "unannotated/agent:latest"
		_, err := store.StoreArtifact(agentImage(t, false, "unannotated"), ref)
		require.NoError(t, err)

		_, _, ok := readCached(store, ref)
		assert.False(t, ok, "cached artifact without cagent annotation must not be served")
	})

	t.Run("annotated entry is returned", func(t *testing.T) {
		ref := "annotated/agent:latest"
		digest, err := store.StoreArtifact(agentImage(t, true, "hello"), ref)
		require.NoError(t, err)

		data, gotDigest, ok := readCached(store, ref)
		require.True(t, ok)
		assert.Equal(t, "hello", string(data))
		assert.Equal(t, digest, gotDigest)
	})
}

// startTestRegistry spins up an in-memory OCI registry on a random port
// and returns its host:port.
func startTestRegistry(t *testing.T) string {
	t.Helper()
	quietLogger := log.New(io.Discard, "", 0)
	srv := httptest.NewServer(registry.New(registry.Logger(quietLogger)))
	t.Cleanup(srv.Close)
	return strings.TrimPrefix(srv.URL, "http://")
}

// pushImageToRegistry pushes img to the given ref on the test registry.
func pushImageToRegistry(t *testing.T, ref string, img v1.Image) {
	t.Helper()
	require.NoError(t, crane.Push(img, ref, crane.Insecure))
}

// TestPull_RecoversFromUnannotatedCache verifies that an unannotated
// cached entry does not permanently block re-pulling a tag whose remote
// content is now a valid agent artifact. Before the fix, Pull would
// short-circuit on the bad cache entry and refuse to contact the
// registry, leaving the user stuck until they manually wiped the store.
func TestPull_RecoversFromUnannotatedCache(t *testing.T) {
	// Not parallel: t.Setenv on HOME mutates process state.

	registryHost := startTestRegistry(t)
	ref := registryHost + "/agent/recovery:latest"

	// Sandbox the content store under a fake HOME.
	t.Setenv("HOME", t.TempDir())

	store, err := content.NewStore()
	require.NoError(t, err)

	// Seed the cache under the *normalised* key, since that is the key
	// Pull itself uses (it strips the registry host).
	seedKey, err := NormalizeReference(ref)
	require.NoError(t, err)
	_, err = store.StoreArtifact(agentImage(t, false, "old, not an agent"), seedKey)
	require.NoError(t, err)

	// Push a properly annotated artifact to the same tag.
	pushImageToRegistry(t, ref, agentImage(t, true, "new agent yaml"))

	// Pull must reach the registry and replace the bad cache entry,
	// not error out on the local metadata.
	digest, err := Pull(t.Context(), ref, false, crane.Insecure)
	require.NoError(t, err)
	require.NotEmpty(t, digest)

	// The cached YAML must now match the freshly-pulled, annotated artifact.
	// Pull normalises the reference (strips the registry host) before keying
	// the store, so we look up the same way.
	storeKey, err := NormalizeReference(ref)
	require.NoError(t, err)
	data, err := store.GetArtifact(storeKey)
	require.NoError(t, err)
	assert.Equal(t, "new agent yaml", data)
}

// TestPullAgent_DigestRefDoesNotServeUnannotatedCache verifies the
// defense-in-depth check on the cache fast-path: even for an immutable
// digest reference, an unannotated cached entry must not be returned.
func TestPullAgent_DigestRefDoesNotServeUnannotatedCache(t *testing.T) {
	// Not parallel: t.Setenv on HOME mutates process state.

	t.Setenv("HOME", t.TempDir())

	store, err := content.NewStore()
	require.NoError(t, err)

	// Seed an unannotated artifact under a tag, then derive a digest ref
	// for it. PullAgent's fast path operates on digest references.
	tagRef := "unannotated/digest:latest"
	digest, err := store.StoreArtifact(agentImage(t, false, "poisoned"), tagRef)
	require.NoError(t, err)

	digestRef := "unannotated/digest@" + digest

	// PullAgent will fall through to remote.Pull, which will fail because
	// the test runs offline. The important property is that the cached
	// bytes are NOT returned silently.
	_, _, err = PullAgent(t.Context(), digestRef, false)
	require.Error(t, err, "unannotated cache hit must not satisfy a digest-pinned PullAgent")
}
