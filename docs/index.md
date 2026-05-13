---
layout: default
title: "Docker Agent"
description: "Run AI agents like containers. Define them in YAML, share them through any OCI registry, and run them anywhere."
permalink: /
---

<div class="hero">
  <h1>Docker Agent</h1>
  <p><strong>Docker Agent is to AI agents what <code>docker run</code> is to containers.</strong> Define an agent in a YAML file, run it with one command, share it through any OCI registry — the same workflow you already use for images.</p>
  <div class="hero-buttons">
    <a href="{{ '/getting-started/quickstart/' | relative_url }}" class="btn btn-primary">Quick Start →</a>
    <a href="https://github.com/docker/docker-agent" target="_blank" rel="noopener noreferrer" class="btn btn-secondary">View on GitHub</a>
  </div>
</div>

<div class="demo-container">
  <img src="{{ '/demo.gif' | relative_url }}" alt="Docker Agent TUI demo showing an interactive agent session" loading="lazy">
</div>

<div class="elevator">
  <div class="elevator-card">
    <div class="elevator-label">What it is</div>
    <p>A CLI that runs AI agents defined declaratively in YAML or HCL.</p>
  </div>
  <div class="elevator-card">
    <div class="elevator-label">What it isn’t</div>
    <p>Not a framework you write code in. Not a hosted SaaS. Not a new model.</p>
  </div>
  <div class="elevator-card">
    <div class="elevator-label">Who it’s for</div>
    <p>Developers who want agents in their workflow without glue code.</p>
  </div>
  <div class="elevator-card">
    <div class="elevator-label">What you get</div>
    <p>TUI · CLI · HTTP API · MCP server · A2A · OCI distribution.</p>
  </div>
</div>

## What Is Docker Agent?

Docker Agent is an open-source tool from **Docker** — makers of [Docker Engine](https://www.docker.com/products/container-runtime/), [Docker Desktop](https://www.docker.com/products/docker-desktop/), [Docker Hub](https://hub.docker.com), and [Docker Scout](https://www.docker.com/products/docker-scout/) — that lets you **build, run, and share AI agents using simple configuration files** instead of writing application code.

You describe what your agent does — its model, personality, tools, and teammates — in a YAML file. Docker Agent handles the LLM orchestration loop, tool execution, multi-agent delegation, and streaming output. You focus on *what* the agent should do, not *how* to wire it up.

```yaml
# agent.yaml — this is all you need
agents:
  root:
    model: anthropic/claude-sonnet-4-5
    description: A coding assistant
    instruction: |
      You are an expert developer. Help users write clean,
      efficient code. Explain your reasoning step by step.
    toolsets:
      - type: filesystem
      - type: shell
      - type: think
```

```bash
$ docker agent run agent.yaml
```

That's it. Your agent can now read and write files, run shell commands, and reason through problems — all through an interactive terminal UI.

## Without vs. with Docker Agent

The same coding assistant, written two different ways:

<div class="compare">
  <div class="compare-side compare-without">
    <div class="compare-label">Without Docker Agent</div>

```python
# ~30 lines of glue code, every project
import anthropic, json, subprocess
from pathlib import Path

client = anthropic.Anthropic()
MODEL = "claude-sonnet-4-5"
TOOLS = [
  {"name": "read_file", "input_schema": {...}},
  {"name": "write_file", "input_schema": {...}},
  {"name": "run_shell", "input_schema": {...}},
]

def dispatch(name, args):
  if name == "read_file":
      return Path(args["path"]).read_text()
  if name == "write_file":
      Path(args["path"]).write_text(args["content"])
      return "ok"
  if name == "run_shell":
      return subprocess.check_output(args["cmd"], shell=True).decode()

messages = [{"role": "user", "content": input("> ")}]
while True:
  resp = client.messages.create(
    model=MODEL, max_tokens=4096, tools=TOOLS,
    system="You are an expert developer…",
    messages=messages,
  )
  # …parse tool_use blocks, dispatch, append, loop…
  if resp.stop_reason == "end_turn": break
```
  </div>
  <div class="compare-side compare-with">
    <div class="compare-label">With Docker Agent</div>

```yaml
# agent.yaml — 8 lines, no glue code
agents:
  root:
    model: anthropic/claude-sonnet-4-5
    description: A coding assistant
    instruction: You are an expert developer.
    toolsets:
      - type: filesystem
      - type: shell
```

```bash
$ docker agent run agent.yaml
```
  </div>
</div>

## Why Docker Agent?

Most AI agent frameworks ask you to write Python or TypeScript to glue together models, tools, and workflows. Docker Agent takes a different approach: **declare everything in config, run it with a single command.**

<div class="pain-grid">
  <div class="pain-row">
    <div class="pain-pain"><span class="pain-x">×</span> “I rebuilt the same agent loop in three projects.”</div>
    <div class="pain-fix"><span class="pain-check">✓</span> Reusable YAML — declare once, run everywhere.</div>
  </div>
  <div class="pain-row">
    <div class="pain-pain"><span class="pain-x">×</span> “Sharing my agent means a repo plus a setup README.”</div>
    <div class="pain-fix"><span class="pain-check">✓</span> <code>docker agent run user/agent</code> — OCI distribution, like images.</div>
  </div>
  <div class="pain-row">
    <div class="pain-pain"><span class="pain-x">×</span> “I’m locked into one model SDK.”</div>
    <div class="pain-fix"><span class="pain-check">✓</span> Swap the <code>model:</code> line — OpenAI, Anthropic, Gemini, Bedrock, local.</div>
  </div>
  <div class="pain-row">
    <div class="pain-pain"><span class="pain-x">×</span> “Tools are one-offs glued to one agent.”</div>
    <div class="pain-fix"><span class="pain-check">✓</span> Built-in toolsets plus 1000+ MCP servers — reuse them across agents.</div>
  </div>
</div>

<div class="features-grid">
  <div class="feature">
    <div class="feature-icon">📝</div>
    <h3>Config, Not Code</h3>
    <p>Define agents in YAML or HCL. Swap models, add tools, or change behavior without touching application code.</p>
  </div>
  <div class="feature">
    <div class="feature-icon">🔧</div>
    <h3>Built-in Tools + MCP</h3>
    <p>Comes with tools for filesystem, shell, memory, web fetch, and more. Extend with any MCP server — over 1,000 are available.</p>
  </div>
  <div class="feature">
    <div class="feature-icon">👥</div>
    <h3>Multi-Agent Teams</h3>
    <p>Build teams of specialized agents that delegate work to each other. A coordinator routes tasks to the right specialist.</p>
  </div>
  <div class="feature">
    <div class="feature-icon">🧠</div>
    <h3>Any Model</h3>
    <p>OpenAI, Anthropic, Google Gemini, AWS Bedrock, local models via Docker Model Runner or Ollama — bring your own provider.</p>
  </div>
  <div class="feature">
    <div class="feature-icon">📦</div>
    <h3>Package &amp; Share Like Images</h3>
    <p>Push agents to any OCI registry. Pull and run them anywhere with one command — the same workflow you use for containers.</p>
  </div>
  <div class="feature">
    <div class="feature-icon">🖥️</div>
    <h3>Run Anywhere</h3>
    <p>Interactive TUI, headless CLI, HTTP API server, OpenAI-compatible chat endpoint, MCP server, or A2A protocol.</p>
  </div>
</div>

## Use Cases

What people build with Docker Agent today:

<div class="usecase-grid">
  <div class="usecase">
    <div class="usecase-icon">⌨️</div>
    <h3>Coding agents</h3>
    <p>Pair-programmer agents with file system, shell, and LSP tools. Read code, edit it, run tests, iterate.</p>
    <a href="https://hub.docker.com/u/agentcatalog" target="_blank" rel="noopener">Browse the catalog →</a>
  </div>
  <div class="usecase">
    <div class="usecase-icon">💻</div>
    <h3>Ops &amp; SRE</h3>
    <p>Triage incidents, search logs, run kubectl, build Dockerfiles. Pipe alerts in via <code>--exec</code> for headless runs.</p>
    <a href="{{ '/features/cli/' | relative_url }}">CLI reference →</a>
  </div>
  <div class="usecase">
    <div class="usecase-icon">📊</div>
    <h3>Data &amp; research</h3>
    <p>Persistent memory, web fetch, RAG over local docs, structured output for downstream pipelines.</p>
    <a href="{{ '/features/rag/' | relative_url }}">RAG guide →</a>
  </div>
  <div class="usecase">
    <div class="usecase-icon">🧭</div>
    <h3>Custom workflows</h3>
    <p>Multi-agent teams, hooks, model routing, A2A and MCP servers — wire agents into your existing stack.</p>
    <a href="{{ '/concepts/multi-agent/' | relative_url }}">Multi-agent →</a>
  </div>
</div>

## How It Works

Docker Agent follows a simple loop:

<figure class="flow-diagram">
  <img src="{{ '/assets/how-it-works.svg' | relative_url }}" alt="agent.yaml is run by 'docker agent run', which loops through Model, Tools and Sub-agents, then streams results to the TUI or API." loading="lazy">
  <figcaption>Your YAML config is the input; the runtime drives a Model ↔ Tools ↔ Sub-agents loop until the task is done; results stream back to the TUI or any API client.</figcaption>
</figure>

1. **You define an agent** in YAML — its model, instructions, tools, and sub-agents
2. **You run it** with `docker agent run` via TUI, CLI, or API
3. **The agent processes your request** — calling tools, delegating to sub-agents, reasoning step by step
4. **Results stream back** in real time

## A few terms you'll see

<dl class="glossary">
  <dt>Agent</dt>
  <dd>An LLM with instructions, tools, and (optionally) sub-agents — the unit you define in YAML.</dd>

  <dt>Tool</dt>
  <dd>A function the agent can call, like <code>read_file</code> or <code>run_shell</code>. Tools come from built-in toolsets or external MCP servers.</dd>

  <dt>MCP</dt>
  <dd><em>Model Context Protocol</em> — an open standard for tool servers. Docker Agent can use any MCP server as a toolset.</dd>

  <dt>A2A</dt>
  <dd><em>Agent-to-Agent</em> — an HTTP protocol agents use to talk to each other across machines.</dd>

  <dt>TUI</dt>
  <dd><em>Terminal User Interface</em> — the default interactive front end, launched by <code>docker agent run</code>.</dd>

  <dt>OCI</dt>
  <dd>The same registry format used for Docker images. Docker Agent reuses it to push and pull agents.</dd>
</dl>

### Zero Config

The fastest way to try it — no config file needed:

```bash
# Run the built-in default agent
$ docker agent run
```

### From the Registry

Run pre-built agents from the [agent catalog on **Docker Hub**](https://hub.docker.com/u/agentcatalog) — just like pulling a Docker image:

```bash
# A pirate-themed assistant
$ docker agent run agentcatalog/pirate

# A coding agent
$ docker agent run agentcatalog/coder
```

### Multi-Agent Teams

Build a team where a coordinator delegates tasks to specialists:

```yaml
agents:
  root:
    model: openai/gpt-5
    description: Team coordinator
    instruction: Route tasks to the best specialist.
    sub_agents: [coder, reviewer]

  coder:
    model: anthropic/claude-sonnet-4-5
    description: Writes and modifies code
    instruction: Write clean, tested code.
    toolsets:
      - type: filesystem
      - type: shell

  reviewer:
    model: anthropic/claude-sonnet-4-5
    description: Reviews code for quality
    instruction: Review code for bugs, style, and best practices.
    toolsets:
      - type: filesystem
```

### Non-Interactive Mode

Use `--exec` for scripting and automation:

```bash
# One-shot task
$ docker agent run --exec agent.yaml "Create a Dockerfile for a Node.js app"

# Pipe input
$ cat error.log | docker agent run --exec agent.yaml "What's wrong in this log?"

# Serve as an API
$ docker agent serve api agent.yaml --listen :8080
```

<div class="callout callout-tip" markdown="1">
<div class="callout-title">Prefer HCL?
</div>
  <p>You can also write agent configs in HCL using labeled blocks and heredocs. See <a href="{{ '/configuration/hcl/' | relative_url }}">HCL Configuration</a>.</p>
</div>

## Part of the Docker ecosystem

Docker Agent reuses the tooling and conventions you already know:

<div class="ecosystem">
  <a class="ecosystem-tile" href="https://hub.docker.com/u/agentcatalog" target="_blank" rel="noopener">
    <strong>Docker Hub</strong>
    <span>Pull pre-built agents from the agent catalog — same registry, same auth.</span>
  </a>
  <a class="ecosystem-tile" href="https://www.docker.com/products/docker-desktop/" target="_blank" rel="noopener">
    <strong>Docker Desktop</strong>
    <span>Run MCP toolsets in containers via <code>ref: docker:…</code> with one click.</span>
  </a>
  <a class="ecosystem-tile" href="https://docs.docker.com/desktop/features/model-runner/" target="_blank" rel="noopener">
    <strong>Docker Model Runner</strong>
    <span>Run local OSS models on your machine — just point your agent at <code>dmr/…</code>.</span>
  </a>
  <a class="ecosystem-tile" href="https://www.docker.com/products/docker-scout/" target="_blank" rel="noopener">
    <strong>Docker Scout</strong>
    <span>Same supply-chain visibility you have for images extends to agent images.</span>
  </a>
</div>

## Explore the Docs

<div class="cards">
  <a class="card" href="{{ '/getting-started/introduction/' | relative_url }}">
    <div class="card-icon">🚀</div>
    <h3>Introduction</h3>
    <p>The full story: what Docker Agent is, why it exists, and how it works.</p>
  </a>
  <a class="card" href="{{ '/getting-started/quickstart/' | relative_url }}">
    <div class="card-icon">⚡</div>
    <h3>Quick Start</h3>
    <p>Get your first agent running in under 5 minutes.</p>
  </a>
  <a class="card" href="{{ '/concepts/agents/' | relative_url }}">
    <div class="card-icon">💡</div>
    <h3>Core Concepts</h3>
    <p>Agents, models, tools, and multi-agent orchestration explained.</p>
  </a>
  <a class="card" href="{{ '/configuration/overview/' | relative_url }}">
    <div class="card-icon">⚙️</div>
    <h3>Configuration</h3>
    <p>Full reference for every YAML and HCL option.</p>
  </a>
  <a class="card" href="{{ '/providers/overview/' | relative_url }}">
    <div class="card-icon">🧠</div>
    <h3>Model Providers</h3>
    <p>OpenAI, Anthropic, Gemini, Bedrock, Docker Model Runner, and more.</p>
  </a>
  <a class="card" href="{{ '/features/tui/' | relative_url }}">
    <div class="card-icon">✨</div>
    <h3>Features</h3>
    <p>TUI, CLI, API server, MCP mode, A2A, RAG, Skills, and distribution.</p>
  </a>
</div>
