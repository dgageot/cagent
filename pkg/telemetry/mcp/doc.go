// Package mcp provides OpenTelemetry instrumentation helpers for the
// Model Context Protocol
// (https://opentelemetry.io/docs/specs/semconv/gen-ai/mcp/).
//
// MCP attributes use the `mcp.*` namespace; trace context propagates
// through the MCP `params._meta` field. All helpers are no-op-safe.
package mcp
