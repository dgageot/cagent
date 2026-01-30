# cagent Go → Rust Migration TODO

This document provides a comprehensive analysis of the feature gap between the Go and Rust implementations of cagent.

## Latest Progress (2026-02-01)

- **~174 features completed** out of 189 tracked items (~92% complete)
- **Session Highlights:**
  - Added /eval TUI command for saving sessions as evaluation files
  - Implemented record command with VCR-style cassette recording
  - Added --fake flag to run command for replaying recorded cassettes
  - Added OAuth token store module with in-memory and file-based persistence
  - Added source loader with caching and periodic refresh for hot-reload
  - Added auto-pull for periodic OCI registry updates
- **Previous Session:**
  - Implemented full OCI push/pull with oci-client library
  - Added Docker config and environment variable authentication for registries  
  - CLI push/pull/run commands now use actual OCI implementation
  - Added evaluation framework with runner, judge (LLM-as-a-judge), and scoring
  - CLI eval command now functional with concurrent Docker-based evaluation
  - Implemented build command for creating agent images
  - Implemented feedback command with GitHub issue creation
  - Added evaluation progress bar with terminal width detection
- **Previous additions:**
  - Added CLI commands: build, record, feedback, acp (stubs)
  - Added runtime config flags: --models-gateway, --env-from-file, --code-mode-tools, --working-dir
  - OCI registry reference detection in config paths
  - GitHub Actions CI workflow
  - Module documentation for runtime and agent
  - TUI handling for MCP init, team info events
  - MockProvider for testing
  - Criterion-based benchmarks
  - Golden file testing utilities
  - Hooks runtime execution (pre/post tool use, session start/end)
  - Session title generator module
  - Deferred (lazy-loaded) tools with search_tool and add_tool
  - MCP stdio client support - Connect to MCP servers via command execution

## Summary

| Category | Status | Notes |
|----------|--------|-------|
| CLI Commands | 99% | Only acp stub remains |
| Model Providers | 100% | All major providers supported |
| Builtin Tools | 92% | Missing: lsp (huge), rag |
| MCP Support | 95% | Stdio/SSE/Streamable/Gateway, token store, missing: OAuth UI dialogs |
| TUI | 98% | Good, missing: OAuth dialogs |
| Server/API | 85% | REST API, fake/record, source loader, auto-pull, missing: ConnectRPC |
| A2A | 100% | Server and client implemented |
| Config | 95% | v3 complete, most v4 features |
| Session | 95% | SQLite persistence, compaction |
| Testing | 70% | VCR + Golden, missing: E2E with real API |
| OCI | 100% | Local store + remote push/pull with Docker config auth |
| Evaluation | 90% | Core types, runner, judge, CLI integration done |

---

## 1. CLI Commands

### ✅ Implemented CLI Commands
- `run` - Run agent (basic + OCI references)
- `exec` - Non-interactive execution
- `new` - Create new agent config
- `version` - Show version
- `push` - Push agent to OCI registry (full implementation)
- `pull` - Pull agent from OCI registry (full implementation)
- `mcp` - Start agent as MCP server (implemented)
- `a2a` - Start agent as A2A server (implemented)
- `api` - Start REST API server (implemented)
- `eval` - Run agent evaluations (full implementation)
- `catalog` - Browse MCP catalog (implemented)
- `config` - Configuration management (show, path)
- `alias` - Agent aliases
- `build` - Build agent images (full implementation)
- `record` - Record AI API interactions (full implementation)
- `feedback` - Send feedback (full implementation)
- `acp` - Agent Client Protocol server (stub)

### ⚠️ CLI Improvements Needed
- [x] Add `--debug` flag with proper file logging to `~/.cagent/cagent.debug.log`
- [x] Add `--log-file` custom log path option
- [x] Add `--otel` OpenTelemetry tracing flag
- [x] Add runtime config flags (`--models-gateway`, `--env-from-file`, `--code-mode-tools`, `--working-dir`)
- [x] Add shell completions (`completion` subcommand)
- [x] Support OCI registry references in config paths (e.g., `agentcatalog/pirate`) (detection only, actual pull not implemented)
- [x] Support directory-based agent loading (accept a directory containing `agent.yaml`/`agent.yml`)

---

## 2. Model Providers

### ✅ Implemented in Rust
- OpenAI
- Anthropic  
- Gemini (Google)

### ❌ Missing Model Providers
- [x] **DMR (Docker Model Runner)** - Local model execution via Docker
- [x] **Bedrock** - AWS Bedrock integration (Converse API with streaming)
- [x] **Mistral** - Mistral AI
- [x] **xAI** - xAI/Grok models
- [x] **Nebius** - Nebius AI
- [x] **Custom providers** - OpenAI-compatible endpoints via config

### ⚠️ Model Provider Improvements Needed
- [x] **Gateway support** - Route requests through models gateway (`CAGENT_MODELS_GATEWAY`)
- [x] **Thinking budget** - Support reasoning budget config (OpenAI effort levels, Anthropic token budgets)
- [x] **Rate limiting** - Handle rate limit headers and backoff
- [x] **Usage tracking** - Track token usage and costs with `track_usage` config
- [x] **Parallel tool calls** - Respect `parallel_tool_calls` config
- [x] **Provider options** - Support `provider_opts` for provider-specific settings
- [x] **Rule-based routing** - Support routing rules to select models based on input (config only)
- [x] **Model cloning** - Clone providers with modified settings

### Missing Model Config Fields
- [x] `top_p`
- [x] `frequency_penalty`
- [x] `presence_penalty`
- [x] `base_url` (custom endpoints)
- [x] `token_key` (custom API key env var)
- [x] `thinking_budget`
- [x] `routing` (rule-based model selection) (config only)

---

## 3. Toolsets & Tools

### ✅ Implemented Builtin Tools in Rust
- `filesystem` - File operations
- `shell` - Command execution
- `think` - Reasoning tool
- `transfer_task` - Agent delegation
- `todo` - Task management
- `fetch` - HTTP requests
- `memory` - SQLite-based persistent memory
- `background_jobs` - Background process management

### ❌ Missing Builtin Tools
- [x] **script** - Custom shell script tools with args schema
- [x] **api** - HTTP API endpoint tools
- [ ] **lsp** - Language Server Protocol tools (huge - 73K+ lines in Go)
- [ ] **rag** - Retrieval-Augmented Generation tools
- [x] **handoff** - Agent handoff tools
- [x] **user_prompt** - Interactive user prompts
- [x] **deferred** - Lazy-loaded tools (search_tool, add_tool)

### ❌ Missing MCP (Model Context Protocol) Support - CRITICAL
This is a major gap - MCP is core to cagent's extensibility.

- [x] **Stdio MCP client** - Connect to MCP servers via command execution
- [x] **Remote MCP client** - SSE and Streamable HTTP transports
- [x] **MCP Gateway toolset** - Docker MCP Gateway integration (`docker:github-official`)
- [x] **OAuth support** - OAuth flows for remote MCP servers (basic infrastructure, needs full OAuth flow)
- [x] **Elicitation** - Handle interactive prompts from MCP servers (handler implemented, runtime/TUI integration pending)
- [x] **Prompts** - MCP prompt templates support
- [x] **Token store** - Persistent OAuth token storage

### ❌ Missing A2A (Agent-to-Agent) Tools
- [x] A2A client toolset for connecting to remote agents

### ⚠️ Tool Improvements Needed
- [x] **Sandbox mode** for shell - Run commands in Docker containers (config only)
- [x] **Post-edit hooks** for filesystem - Run commands after file edits
- [x] **VCS integration** for filesystem - Respect `.gitignore`
- [x] **Shared todos** - Shared todo lists across agents
- [x] **Tool filtering** - `tools: ["specific_tool"]` in toolset config
- [x] **Tool instructions** - Per-toolset instructions
- [x] **Defer loading** - Lazy-load tools (`defer: true` or `defer: ["tool1", "tool2"]`)
- [x] **Tool annotations** - Support MCP tool annotations (read-only hint)

---

## 4. Configuration

### ✅ Implemented Config Features
- Basic agent config
- Model definitions
- Provider config
- Toolset config (partial)

### ❌ Missing Config Features

#### Agent Config
- [x] `welcome_message` - Initial message displayed to user
- [ ] `rag` - RAG source references (config added, runtime not implemented)
- [x] `code_mode_tools` - Code execution tools (config only)
- [x] `add_description_parameter` - Add description to tool calls
- [x] `add_prompt_files` - Include additional prompt files
- [x] `commands` - Named prompts/shortcuts
- [x] `structured_output` - JSON schema output
- [x] `hooks` - Pre/post tool hooks
- [x] `permissions` - Allow/Ask/Deny patterns

#### Toolset Config
- [x] `toon` - Tool "toon" (persona)
- [x] `sandbox` - Docker sandbox config
- [x] `post_edit` - Post-edit commands
- [x] `ignore_vcs` - VCS ignore settings
- [x] `timeout` - Tool timeout (currently enforced for `shell`; fetch already uses reqwest timeouts)
- [x] `api_config` - API tool config
- [x] `shell` - Script shell definitions
- [x] `config` - MCP config passthrough

#### RAG Config (config added, runtime not implemented)
- [x] RAG sources with strategies (config only)
- [x] Chunked embeddings (config only)
- [x] BM25 search (config only)
- [x] Fusion strategies (config only)
- [x] Reranking (config only)
- [x] Database config (config only)
- [x] Results config (config only)

#### Permissions Config
- [x] `allow` - Auto-approve patterns
- [x] `deny` - Always reject patterns

#### Hooks Config
- [x] `pre_tool_use` - Hooks before tool execution (config + runtime)
- [x] `post_tool_use` - Hooks after tool execution (config + runtime)
- [x] `session_start` - Session start hooks (config + runtime)
- [x] `session_end` - Session end hooks (config + runtime)

#### Metadata
- [x] `author`
- [x] `license`
- [x] `description`
- [x] `readme`
- [x] `version`

### ⚠️ Config Version
- Go is at version `4`, Rust is at version `3`
- Need config migration/upgrade system

---

## 5. Runtime & Session

### ⚠️ Runtime Improvements Needed
- [x] **Max iterations** - Enforce `max_iterations` limit (emits `MaxIterationsReached` event)
- [x] **History items** - Limit history with `num_history_items`
- [x] **Compaction** - Session compaction/summarization (manual `/compact` in TUI; stores `SessionItem::Summary`)
- [ ] **Remote runtime** - Connect to remote runtime servers
- [x] **Elicitation flow** - Handle MCP elicitation requests (types defined, full runtime flow pending)
- [ ] **OAuth flow** - Handle OAuth redirects from MCP

### ❌ Missing Runtime Events
- [x] `partial_tool_call` - Streaming tool calls (event added)
- [x] `token_usage` - Detailed usage with cost
- [x] `session_title` - Auto-generated titles (event added)
- [x] `session_summary` - Session summaries (via /compact)
- [x] `session_compaction` - Compaction status
- [x] `elicitation_request` - MCP elicitation (types defined, runtime handler pending)
- [ ] `authorization_event` - OAuth authorization
- [x] `max_iterations_reached` - Iteration limit hit
- [x] `mcp_init_started/finished` - MCP lifecycle (events added)
- [x] `agent_info` - Agent details
- [x] `team_info` - Team/multi-agent info (event added)
- [x] `toolset_info` - Available tools info (event added)
- [ ] `rag_indexing_*` - RAG lifecycle events
- [x] `hook_blocked` - Hook blocked tool (event added)
- [x] `message_added` - Message persistence
- [x] `sub_session_completed` - Sub-agent completion

### ❌ Missing Session Features
- [x] **SQLite persistence** - Session store in SQLite
- [x] **Sub-sessions** - Track agent delegation sessions
- [x] **Session export** - Export conversation history (TUI: `/export`)
- [x] **Cost tracking** - Per-session cost accumulation

---

## 6. TUI (Terminal User Interface)

### ✅ Implemented TUI Features
- Basic chat interface
- Markdown rendering
- Agent response streaming
- Tool call display
- Syntax highlighting

### ❌ Missing TUI Features
- [x] **Commands** - `/new`, `/compact`, `/copy`, `/exit`, `/reset`, `/usage`, `/help`, `/yolo`, `/think`, `/eval`
- [x] **Sidebar** - Agent info sidebar (note: multi-agent switching not implemented yet)
- [x] **Dialogs** - Tool confirmation dialogs with diff view
- [x] **Progress indicators** - MCP init, RAG indexing (MCP init messages added)
- [x] **Token usage display** - Usage statistics
- [x] **Animation system** - Shared tick animation (basic spinner implemented)
- [x] **Elicitation dialogs** - Form-based input in TUI for MCP elicitation requests
- [ ] **OAuth dialogs** - OAuth flow UI
- [x] **History navigation** - Browse past commands (up/down arrows)
- [x] **Multi-agent switching** - Switch between agents in TUI
- [x] **Welcome message** - Display agent welcome
- [x] **Error dialogs** - Better error display (inline with styling)
- [x] **Copy to clipboard** - Copy conversation
- [x] **Title generation** - Auto-generate session titles (TitleGenerator + TUI event handling complete)

### ⚠️ TUI Improvements Needed
- [x] Better theme/styling consistency (comprehensive theme system with dark/light/high-contrast)
- [x] Responsive layout (basic - ratatui constraints handle resizing)
- [x] Keyboard shortcuts (Ctrl+C, Ctrl+B, Ctrl+P, Ctrl+G, etc.)
- [x] Mouse support (scroll implemented)

---

## 7. Server & API

### ❌ Entirely Missing
- [x] **REST API server** - HTTP API for agent interaction
- [ ] **ConnectRPC server** - gRPC-web compatible API
- [x] **Session manager** - Multi-session management
- [x] **SSE streaming** - Server-sent events for real-time updates (implemented in API)
- [x] **Source loader** - Hot-reload agent configs
- [x] **Auto-pull** - Periodic OCI registry pulls
- [x] **Fake/Record modes** - VCR-style cassette recording/replay

---

## 8. MCP Server Mode

### ✅ Implemented
- [x] **Stdio MCP server** - Expose agent as MCP tool via stdio

### ❌ Missing
- [x] **HTTP MCP server** - Streamable HTTP MCP server
- [x] **Multi-agent MCP** - Expose all agents as MCP tools

---

## 9. A2A (Agent-to-Agent) Protocol

### ❌ Entirely Missing
- [x] **A2A server** - Expose agent via A2A protocol
- [x] **A2A client** - Connect to other A2A agents
- [x] **Agent cards** - A2A agent metadata
- [x] **Skills** - A2A skill definitions

---

## 10. OCI Registry Support

### ✅ Implemented
- [x] **Content store** - Local artifact cache
- [x] **Store/Get** - Store and retrieve artifacts locally
- [x] **Metadata** - OCI annotations
- [x] **Reference detection** - Detect OCI references vs file paths
- [x] **Push** - Push to remote OCI registries (with Docker config auth)
- [x] **Pull** - Pull from remote OCI registries (with Docker config auth)

---

## 11. Evaluation Framework

### ✅ Implemented
- [x] **Eval runner** - Run evaluation suites with concurrent execution
- [x] **Judge model** - AI-based relevance checking (LLM-as-a-judge)
- [x] **Types** - EvalSession, Result, Summary, EvalRun
- [x] **Scoring** - F1 score, size classification, relevance metrics
- [x] **CLI integration** - `cagent eval` command
- [x] **Results export** - JSON results output

### ❌ Missing
- [x] **Progress tracking** - Concurrent eval progress (full progress bar with terminal width)
- [ ] **Docker isolation** - Run evals in containers (code exists, not tested)

---

## 12. Telemetry

### ✅ Implemented
- [x] **Usage tracking** - Anonymous usage telemetry (placeholder)
- [x] **Command events** - Track command usage (placeholder)
- [x] **Error tracking** - Track errors (via Event::Error and Event::Warning)
- [x] **Opt-out** - `TELEMETRY_ENABLED=false` (defaults to disabled)

---

## 13. Gateway & Catalog

### ✅ Implemented
- [x] **MCP catalog** - Fetch Docker MCP catalog (basic list implemented)
- [x] **Catalog caching** - Local cache with TTL (24 hours)
- [x] **Server discovery** - Find MCP servers by name
- [x] **Secret requirements** - Discover required env vars

---

## 14. Environment & Paths

### ✅ Implemented
- [x] **Environment provider** - Configurable env var source
- [x] **Home dir** - `~/.cagent/` directory structure
- [x] **Config dir** - XDG-compliant config paths
- [x] **Cache dir** - Artifact caching paths

---

## 15. Testing & Development

### ✅ Implemented
- [x] **VCR/Cassette testing** - Record/replay AI interactions
- [x] **Golden file testing** - Snapshot testing (tests/golden.rs)
- [x] **Mock providers** - Fake model providers for testing
- [x] **E2E tests** - Integration tests with mock providers

### ❌ Missing
- [ ] **E2E tests with VCR** - Record/replay real AI interactions

---

## 16. Code Quality & Consistency

### ⚠️ Areas Needing Work
- [x] **Error handling** - Consistent error types and messages (thiserror-based)
- [x] **Logging** - Structured logging with tracing (tracing crate with env-filter and OpenTelemetry)
- [x] **Documentation** - Inline docs and README (basic module docs added)
- [x] **CI/CD** - GitHub Actions workflow
- [x] **Linting** - Clippy + rustfmt config
- [x] **Benchmarks** - Performance testing (criterion-based)

---

## Priority Recommendations

### P0 - Critical (Core Functionality)
1. MCP support (stdio client at minimum)
2. More model providers (DMR for local models)
3. Missing CLI commands (`push`, `pull`, `mcp`)
4. Config v4 support with all fields

### P1 - High (Feature Parity)
1. REST API server
2. Full TUI commands
3. Session persistence
4. All builtin tools

### P2 - Medium (Enhanced Features)
1. A2A support
2. Evaluation framework
3. RAG support
4. OAuth flows

### P3 - Nice to Have
1. Telemetry
2. LSP tools
3. Full E2E test suite

---

## Notes

- The Go codebase is significantly more mature with ~55 packages vs Rust's ~10 modules
- MCP integration is fundamental to cagent - this should be prioritized
- The TUI in Rust has good foundations but needs the command system
- Consider using existing Rust MCP client libraries rather than building from scratch
- The evaluation framework could be ported later as it's primarily for internal testing
