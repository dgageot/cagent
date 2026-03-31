#!/usr/bin/env bash
#
# npm/scripts/package.sh
#
# Prepares npm packages for publishing by:
#   1. Copying cross-compiled binaries from dist/ into the platform packages
#   2. Stamping all package.json files with the given version
#   3. Copying LICENSE and README into the main package
#
# Usage: ./npm/scripts/package.sh <version>
#
# Expects the dist/ directory to contain cross-compiled binaries produced
# by `task cross` (docker buildx --output=./dist), with the structure:
#   dist/<goos>_<goarch>/docker-agent-<goos>-<goarch>       (unix)
#   dist/<goos>_<goarch>/docker-agent-<goos>-<goarch>.exe   (windows)

set -euo pipefail

VERSION="${1:?Usage: $0 <version>}"

# Validate version to prevent sed injection
if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9.]+)?$ ]]; then
  echo "ERROR: Invalid version format: $VERSION"
  echo "       Expected semver (e.g., 1.2.3 or 1.2.3-beta.1)"
  exit 1
fi
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
NPM_DIR="$ROOT_DIR/npm"
DIST_DIR="$ROOT_DIR/dist"

# Mapping: npm-platform-arch -> dist-subdir/binary-name -> npm-binary-name
#   key:   npm platform suffix (e.g., darwin-arm64)
#   value: "<dist_subdir>/<src_binary>|<dest_binary>"
declare -A PLATFORM_MAP=(
  ["darwin-arm64"]="darwin_arm64/docker-agent-darwin-arm64|docker-agent"
  ["darwin-x64"]="darwin_amd64/docker-agent-darwin-amd64|docker-agent"
  ["linux-arm64"]="linux_arm64/docker-agent-linux-arm64|docker-agent"
  ["linux-x64"]="linux_amd64/docker-agent-linux-amd64|docker-agent"
  ["win32-arm64"]="windows_arm64/docker-agent-windows-arm64.exe|docker-agent.exe"
  ["win32-x64"]="windows_amd64/docker-agent-windows-amd64.exe|docker-agent.exe"
)

echo "Packaging npm packages for version ${VERSION}..."

# --- Step 1: Copy binaries into platform packages ---
for npm_platform in "${!PLATFORM_MAP[@]}"; do
  mapping="${PLATFORM_MAP[$npm_platform]}"
  src_rel="${mapping%%|*}"
  dest_name="${mapping##*|}"
  src="${DIST_DIR}/${src_rel}"
  pkg_dir="${NPM_DIR}/docker-agent-${npm_platform}"

  if [ ! -f "$src" ]; then
    echo "ERROR: Binary not found: $src"
    echo "       Run 'task cross' first to build all platform binaries."
    exit 1
  fi

  echo "  Copying $src -> $pkg_dir/$dest_name"
  cp "$src" "$pkg_dir/$dest_name"
  chmod +x "$pkg_dir/$dest_name"
done

# --- Step 2: Stamp version in all package.json files ---
echo "  Setting version to ${VERSION} in all package.json files..."
for pkg_json in "$NPM_DIR"/*/package.json; do
  sed -i.bak "s/\"0.0.0\"/\"${VERSION}\"/g" "$pkg_json"
  rm -f "${pkg_json}.bak"
done

# --- Step 3: Copy LICENSE and README into main package ---
cp "$ROOT_DIR/LICENSE" "$NPM_DIR/docker-agent/LICENSE"
cp "$ROOT_DIR/README.md" "$NPM_DIR/docker-agent/README.md"

echo ""
echo "Done. Packages ready in ${NPM_DIR}/"
echo ""
echo "To publish (platform packages first, then main package):"
echo "  cd ${NPM_DIR}"
echo "  for pkg in docker-agent-*/; do (cd \"\$pkg\" && npm publish --access public); done"
echo "  cd docker-agent && npm publish --access public"
