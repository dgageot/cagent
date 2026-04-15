package toolinstall

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
)

// npmPrefix is the version prefix that triggers npm-based installation.
const npmPrefix = "npm:"

// isNpmRef returns true if the version string starts with the npm: prefix.
func isNpmRef(version string) bool {
	return strings.HasPrefix(version, npmPrefix)
}

// parseNpmRef parses an npm version reference like "npm:@scope/pkg" or "npm:pkg@1.0.0"
// and returns the npm package name and optional version.
func parseNpmRef(ref string) (pkg, version string, err error) {
	ref = strings.TrimPrefix(ref, npmPrefix)
	ref = strings.TrimSpace(ref)

	if ref == "" {
		return "", "", errors.New("empty npm package reference")
	}

	// Handle scoped packages like @scope/pkg@version
	if strings.HasPrefix(ref, "@") {
		parts := strings.SplitN(ref, "/", 2)
		if len(parts) != 2 || parts[0] == "@" || parts[1] == "" {
			return "", "", fmt.Errorf("invalid scoped npm package %q: expected @scope/name format", ref)
		}

		// Find version separator after the package name
		if idx := strings.Index(parts[1], "@"); idx >= 0 {
			if parts[1][:idx] == "" {
				return "", "", fmt.Errorf("invalid scoped npm package %q: empty package name", ref)
			}
			return parts[0] + "/" + parts[1][:idx], parts[1][idx+1:], nil
		}
		return ref, "", nil
	}

	// Handle unscoped packages like pkg@version
	parts := strings.SplitN(ref, "@", 2)
	if len(parts) == 2 {
		return parts[0], parts[1], nil
	}

	return ref, "", nil
}

// installNpmPackage installs an npm package globally into the tools directory
// and returns the path to the command binary.
func installNpmPackage(ctx context.Context, command, npmRef string) (string, error) {
	npmPkg, npmVersion, err := parseNpmRef(npmRef)
	if err != nil {
		return "", fmt.Errorf("invalid npm reference %q: %w", npmRef, err)
	}

	npmBin, err := exec.LookPath("npm")
	if err != nil {
		return "", errors.New("npm not found in PATH: install Node.js to use npm-based tool installation")
	}

	installArg := npmPkg
	if npmVersion != "" {
		installArg = npmPkg + "@" + npmVersion
	}

	slog.Info("Installing npm package", "command", command, "package", installArg)

	// Use --prefix to install into our tools directory so that the binary
	// ends up in BinDir() (prefix/bin/).
	prefixDir := ToolsDir()
	if err := os.MkdirAll(prefixDir, 0o755); err != nil {
		return "", fmt.Errorf("creating tools directory: %w", err)
	}

	cmd := exec.CommandContext(ctx, npmBin, "install", "--global", "--prefix", prefixDir, installArg)
	cmd.Stdout = os.Stderr
	cmd.Stderr = os.Stderr

	if err := cmd.Run(); err != nil {
		return "", fmt.Errorf("npm install %s failed: %w", installArg, err)
	}

	// Verify the binary was installed.
	binPath := filepath.Join(BinDir(), command)
	if info, err := os.Stat(binPath); err == nil && info.Mode()&0o111 != 0 {
		slog.Info("Successfully installed npm package", "command", command, "package", installArg, "path", binPath)
		return binPath, nil
	}

	return "", fmt.Errorf("binary %q not found after npm install %s (expected at %s)", command, installArg, binPath)
}
