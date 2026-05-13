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

Docker Agent is an open-source tool from Docker that lets you **build, run, and share AI agents using simple configuration files** instead of writing application code.

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

## Why Docker Agent?

Most AI agent frameworks ask you to write Python or TypeScript to glue together models, tools, and workflows. Docker Agent takes a different approach: **declare everything in config, run it with a single command.**

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

## How It Works

Docker Agent follows a simple loop:

1. **You define an agent** in YAML — its model, instructions, tools, and sub-agents
2. **You run it** with `docker agent run` via TUI, CLI, or API
3. **The agent processes your request** — calling tools, delegating to sub-agents, reasoning step by step
4. **Results stream back** in real time

### Zero Config

The fastest way to try it — no config file needed:

```bash
# Run the built-in default agent
$ docker agent run
```

### From the Registry

Run pre-built agents from the [agent catalog](https://hub.docker.com/u/agentcatalog) — just like pulling a Docker image:

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
