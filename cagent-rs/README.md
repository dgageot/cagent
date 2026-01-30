# cagent-rs 🦀

A Rust port of [cagent](https://github.com/docker/cagent) - an AI agent runner with multi-agent support.

## Features

### Core Features
- **Multi-agent support**: Hierarchical agent structure with task delegation via `transfer_task`
- **Streaming responses**: Real-time streaming from AI providers
- **YAML configuration**: Declarative agent and tool definitions
- **Interactive CLI**: Full-featured command-line interface with colors and commands

### Model Providers
- **OpenAI**: GPT-4o, GPT-4, GPT-3.5, etc.
- **Anthropic**: Claude 3.5 Sonnet, Claude 3 Opus, etc.
- **Google Gemini**: Gemini Pro, Gemini Flash, etc.
- **Mistral**: Mistral models
- **xAI**: Grok models
- **Custom providers**: OpenAI-compatible endpoints

### Built-in Tools
- **filesystem**: Read, write, edit files, directory operations, search, with post-edit hooks
- **shell**: Execute shell commands with background job support and timeout
- **think**: Step-by-step reasoning tool
- **todo**: Task tracking (create, update, list todos) - supports shared mode
- **fetch**: HTTP requests with multiple URL support
- **memory**: Persistent SQLite-backed memory storage
- **background_jobs**: Run and manage long-running processes
- **script**: Custom shell script tools with argument schemas
- **api**: HTTP API endpoint tools
- **handoff**: Agent handoff for conversation transfer

## Installation

```bash
cargo install --path .
```

## Quick Start

```bash
# Run with default agent (requires OPENAI_API_KEY)
cagent run

# Run with TUI (full-screen terminal interface)
cagent run --tui

# Run with a configuration file
cagent run ./agent.yaml

# Run with initial message
cagent run ./agent.yaml "Hello, how can you help me?"

# Auto-approve all tool calls
cagent run ./agent.yaml --yolo

# Override model
cagent run ./agent.yaml --model anthropic/claude-sonnet-4-0

# Create a new agent configuration
cagent new --output agent.yaml

# Execute non-interactively
cagent exec ./agent.yaml "List files in current directory" --yolo
```

## Interactive Commands

During a `cagent run` session:

| Command | Description |
|---------|-------------|
| `/help` | Show available commands |
| `/new` | Start a new session |
| `/reset` | Reset the session |
| `/usage` | Show token usage statistics |
| `/copy` | Copy last response to clipboard |
| `/export [file]` | Export conversation to markdown |
| `/compact` | Compact session history |
| `/search <query>` | Search in conversation |
| `/filter <type>` | Filter messages |
| `/theme <name>` | Change color theme |
| `/yolo` | Toggle auto-approve |
| `/exit` | Exit the session |

## Configuration

### Basic Agent
```yaml
version: "3"

agents:
  root:
    model: openai/gpt-4o
    description: A helpful AI assistant
    instruction: |
      You are a helpful AI assistant.
    add_date: true
    add_environment_info: true
    toolsets:
      - type: filesystem
      - type: shell
      - type: think
      - type: todo
      - type: fetch
      - type: memory
        path: ./memory.db  # Optional: custom path
```

### Multi-Agent Team
```yaml
version: "3"

agents:
  root:
    model: openai/gpt-4o
    description: Project coordinator
    sub_agents:
      - researcher
      - developer
    toolsets:
      - type: transfer_task
      - type: think

  researcher:
    model: anthropic/claude-sonnet-4-0
    description: Research specialist
    toolsets:
      - type: fetch
      - type: filesystem

  developer:
    model: openai/gpt-4o
    description: Code developer
    toolsets:
      - type: filesystem
      - type: shell
      - type: think
```

### Named Models
```yaml
version: "3"

models:
  fast:
    provider: openai
    model: gpt-4o-mini
    temperature: 0.7
    max_tokens: 4000

  smart:
    provider: anthropic
    model: claude-sonnet-4-0
    max_tokens: 8192

agents:
  root:
    model: fast  # Reference named model
    # ...
```

## Environment Variables

| Variable | Description |
|----------|-------------|
| `OPENAI_API_KEY` | OpenAI API key |
| `ANTHROPIC_API_KEY` | Anthropic API key |
| `GOOGLE_API_KEY` or `GEMINI_API_KEY` | Google/Gemini API key |

## Architecture

```
cagent-rs/
├── src/
│   ├── main.rs           # Entry point
│   ├── lib.rs            # Library exports
│   ├── agent/            # Agent abstraction & Team management
│   ├── chat/             # Message types
│   ├── cli/              # CLI with clap
│   ├── config/           # YAML configuration parsing
│   ├── model/
│   │   ├── openai.rs     # OpenAI streaming provider
│   │   ├── anthropic.rs  # Anthropic streaming provider
│   │   └── gemini.rs     # Google Gemini provider
│   ├── runtime/          # Event-driven execution engine
│   ├── session/          # Conversation state management
│   └── tools/
│       └── builtin.rs    # All built-in tools
├── examples/
│   ├── basic_agent.yaml
│   └── dev_team.yaml
└── Cargo.toml
```

## Tool Approval

By default, tools that modify state require user approval:

- `y` or `yes` - Approve this tool call
- `a` or `all` - Approve all tool calls for the session
- `n` or `no` - Reject this tool call

Use `--yolo` to auto-approve all tool calls.

## Comparison with Go Version

| Feature | Go | Rust |
|---------|:--:|:----:|
| Multi-agent | ✅ | ✅ |
| OpenAI | ✅ | ✅ |
| Anthropic | ✅ | ✅ |
| Gemini | ✅ | ✅ |
| Mistral | ✅ | ✅ |
| xAI | ✅ | ✅ |
| Bedrock | ✅ | ✅ |
| DMR (Docker Model Runner) | ✅ | ✅ |
| Filesystem tools | ✅ | ✅ |
| Shell tools | ✅ | ✅ |
| Background jobs | ✅ | ✅ |
| Todo tools | ✅ | ✅ |
| Fetch tools | ✅ | ✅ |
| Memory (SQLite) | ✅ | ✅ |
| Think tool | ✅ | ✅ |
| TUI | ✅ | ✅ |
| MCP (Stdio/SSE/Streamable) | ✅ | ✅ |
| A2A protocol | ✅ | ✅ |
| Permissions (allow/deny) | ✅ | ✅ |
| Post-edit hooks | ✅ | ✅ |
| Shared todos | ✅ | ✅ |
| Custom providers | ✅ | ✅ |
| Session persistence | ✅ | ✅ |
| OCI push/pull | ✅ | ✅ |
| Evaluation framework | ✅ | ✅ |
| Record/replay | ✅ | ✅ |
| REST API | ✅ | ✅ |

## License

Apache-2.0
