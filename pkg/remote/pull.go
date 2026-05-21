package remote

import (
	"context"
	"errors"
	"fmt"
	"log/slog"

	"github.com/google/go-containerregistry/pkg/crane"
	"github.com/google/go-containerregistry/pkg/name"

	"github.com/docker/docker-agent/pkg/content"
)

// NormalizeReference parses an OCI reference and returns the normalized
// store key that Pull uses to store artifacts. This ensures that equivalent
// references (e.g. "agentcatalog/review-pr" and
// "index.docker.io/agentcatalog/review-pr:latest") map to the same key.
func NormalizeReference(registryRef string) (string, error) {
	ref, err := name.ParseReference(registryRef)
	if err != nil {
		return "", fmt.Errorf("parsing registry reference %s: %w", registryRef, err)
	}
	return ref.Context().RepositoryStr() + separator(ref) + ref.Identifier(), nil
}

// IsDigestReference reports whether the given reference pins a specific
// image digest (e.g. "repo@sha256:abc...").
func IsDigestReference(registryRef string) bool {
	ref, err := name.ParseReference(registryRef)
	if err != nil {
		return false
	}
	_, ok := ref.(name.Digest)
	return ok
}

// Pull pulls an artifact from a registry and stores it in the content store
func Pull(ctx context.Context, registryRef string, force bool, opts ...crane.Option) (string, error) {
	opts = append(opts, crane.WithContext(ctx), crane.WithTransport(NewTransport(ctx)))

	ref, err := name.ParseReference(registryRef)
	if err != nil {
		return "", fmt.Errorf("parsing registry reference %s: %w", registryRef, err)
	}

	store, err := content.NewStore()
	if err != nil {
		return "", fmt.Errorf("creating content store: %w", err)
	}

	localRef := ref.Context().RepositoryStr() + separator(ref) + ref.Identifier()

	// Cache check: only worth a HEAD round-trip when we actually have a
	// local copy to compare against. When the cache is empty we'd have to
	// pull anyway, so skip the digest probe entirely.
	if !force {
		if meta, metaErr := store.GetArtifactMetadata(localRef); metaErr == nil {
			if !hasCagentAnnotation(meta.Annotations) {
				return "", fmt.Errorf("artifact %s found in store wasn't created by `docker agent share push`\nTry to push again with `docker agent share push`", localRef)
			}
			remoteDigest, err := crane.Digest(ref.String(), opts...)
			if err != nil {
				return "", fmt.Errorf("resolving remote digest for %s: %w", registryRef, err)
			}
			if meta.Digest == remoteDigest {
				return meta.Digest, nil
			}
		}
	}

	img, err := crane.Pull(ref.String(), opts...)
	if err != nil {
		return "", fmt.Errorf("pulling image from registry %s: %w", registryRef, err)
	}

	manifest, err := img.Manifest()
	if err != nil {
		return "", fmt.Errorf("getting manifest from pulled image: %w", err)
	}
	if !hasCagentAnnotation(manifest.Annotations) {
		return "", fmt.Errorf("artifact %s wasn't created by `docker agent share push`\nTry to push again with `docker agent share push`", localRef)
	}

	digest, err := store.StoreArtifact(img, localRef)
	if err != nil {
		return "", fmt.Errorf("storing artifact in content store: %w", err)
	}

	return digest, nil
}

// PullAgent fetches the agent YAML for the given OCI reference and returns its
// bytes and resolved sha256 digest. The local content store is used as a
// cache; the registry remains the source of truth.
//
// Behavior:
//   - Digest refs are served from the cache when present (immutable content).
//   - Tag refs are refreshed against the registry; the cache is reused when
//     the remote digest is unchanged.
//   - On registry/network errors, a previously cached copy is returned if one
//     exists; the error is logged at debug level.
//   - If the local store is detected as corrupted, a single forced re-pull
//     is attempted.
func PullAgent(ctx context.Context, registryRef string, force bool) ([]byte, string, error) {
	storeKey, err := NormalizeReference(registryRef)
	if err != nil {
		return nil, "", err
	}

	store, err := content.NewStore()
	if err != nil {
		return nil, "", fmt.Errorf("creating content store: %w", err)
	}

	// Digest refs are immutable: serve from cache without any network call.
	if !force && IsDigestReference(registryRef) {
		if data, digest, ok := readCached(store, storeKey); ok {
			return data, digest, nil
		}
	}

	digest, pullErr := Pull(ctx, registryRef, force)
	if pullErr != nil {
		// Tolerate registry/network failures when we have a usable cached copy.
		if !force {
			if data, d, ok := readCached(store, storeKey); ok {
				slog.DebugContext(ctx, "Failed to refresh OCI artifact, using cached copy",
					"ref", registryRef, "error", pullErr)
				return data, d, nil
			}
		}
		return nil, "", fmt.Errorf("failed to pull OCI image %s: %w", registryRef, pullErr)
	}

	data, err := store.GetArtifact(storeKey)
	if err == nil {
		return []byte(data), digest, nil
	}

	// Store went missing right after a successful pull. Re-pull once.
	if !errors.Is(err, content.ErrStoreCorrupted) {
		return nil, "", fmt.Errorf("loading artifact from store: %w", err)
	}

	slog.WarnContext(ctx, "Local OCI store corrupted, forcing re-pull", "ref", registryRef)
	if digest, err = Pull(ctx, registryRef, true); err != nil {
		return nil, "", fmt.Errorf("force re-pull %s: %w", registryRef, err)
	}
	data, err = store.GetArtifact(storeKey)
	if err != nil {
		return nil, "", fmt.Errorf("loading artifact from store after re-pull: %w", err)
	}
	return []byte(data), digest, nil
}

// readCached returns the cached YAML bytes and digest for storeKey, or
// ok=false if no usable cached copy is present.
func readCached(store *content.Store, storeKey string) ([]byte, string, bool) {
	meta, err := store.GetArtifactMetadata(storeKey)
	if err != nil {
		return nil, "", false
	}
	data, err := store.GetArtifact(storeKey)
	if err != nil {
		return nil, "", false
	}
	return []byte(data), meta.Digest, true
}

func hasCagentAnnotation(annotations map[string]string) bool {
	_, exists := annotations["io.docker.agent.version"]
	if !exists {
		_, exists = annotations["io.docker.cagent.version"]
	}
	return exists
}

// separator returns the separator used between repository and identifier.
// For digests it returns "@", for tags it returns ":".
func separator(ref name.Reference) string {
	if _, ok := ref.(name.Digest); ok {
		return "@"
	}
	return ":"
}
