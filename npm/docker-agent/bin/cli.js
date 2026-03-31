#!/usr/bin/env node

"use strict";

const { execFileSync } = require("child_process");
const path = require("path");

// Map Node.js platform/arch to npm package names
const PLATFORM_MAP = {
  "darwin-arm64": "@docker/docker-agent-darwin-arm64",
  "darwin-x64": "@docker/docker-agent-darwin-x64",
  "linux-arm64": "@docker/docker-agent-linux-arm64",
  "linux-x64": "@docker/docker-agent-linux-x64",
  "win32-arm64": "@docker/docker-agent-win32-arm64",
  "win32-x64": "@docker/docker-agent-win32-x64",
};

const platformKey = `${process.platform}-${process.arch}`;
const packageName = PLATFORM_MAP[platformKey];

if (!packageName) {
  console.error(
    `Unsupported platform: ${process.platform} ${process.arch}\n` +
      `docker-agent supports: ${Object.keys(PLATFORM_MAP).join(", ")}`
  );
  process.exit(1);
}

const binaryName = process.platform === "win32" ? "docker-agent.exe" : "docker-agent";

let binaryPath;
try {
  // Resolve the platform-specific package and find the binary inside it
  const packageDir = path.dirname(require.resolve(`${packageName}/package.json`));
  binaryPath = path.join(packageDir, binaryName);
} catch {
  console.error(
    `Failed to find package ${packageName}.\n` +
      `This usually means the optional dependency was not installed.\n` +
      `Try reinstalling: npm install @docker/docker-agent`
  );
  process.exit(1);
}

try {
  execFileSync(binaryPath, process.argv.slice(2), {
    stdio: "inherit",
  });
} catch (err) {
  // execFileSync throws on non-zero exit codes; forward the exit code
  if (err.status !== null) {
    process.exit(err.status);
  }
  throw err;
}
