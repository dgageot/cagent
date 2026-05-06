---
title: "Introduction"
description: "docker-agent is a powerful, customizable multi-agent system that lets you build, run, and share AI agents using simple YAML or HCL configuration."
permalink: /getting-started/introduction/
---

# Introduction

_docker-agent is a powerful, customizable multi-agent system that lets you build, run, and share AI agents using simple YAML or HCL configuration._

## What is docker-agent?

docker-agent is an open-source tool by Docker that orchestrates AI agents with specialized capabilities and tools.
Instead of writing code to wire up LLMs, tools, and workflows, you **declare** your agents in YAML or HCL —
their model, personality, tools, and how they collaborate — and docker-agent handles the rest.

<div class="features-grid">
  <div class="feature">
    <div class="feature-icon">🏗️</div>
    <h3>Multi-Agent Architecture</h3>
    <p>Build hierarchical teams of agents that specialize in different tasks and delegate work to each other.</p>

  </div>
  <div class="feature">
    <div class="feature-icon">🔧</div>
    <h3>Rich Tool Ecosystem</h3>
    <p>Built-in tools for files, shell, memory, and todos. Extend with any MCP server — over 1000+ available.</p>

  </div>
  <div class="feature">
    <div class="feature-icon">🧠</div>
    <h3>Multi-Model Support</h3>
    <p>OpenAI, Anthropic, Google Gemini, AWS Bedrock, Docker Model Runner, and reusable provider definitions with shared defaults.</p>

  </div>
  <div class="feature">
    <div class="feature-icon">📦</div>
    <h3>Package &amp; Share</h3>
    <p>Push agents to OCI registries and pull them anywhere — just like Docker images.</p>

  </div>
  <div class="feature">
    <div class="feature-icon">🖥️</div>
    <h3>Multiple Interfaces</h3>
    <p>Interactive TUI, headless CLI, HTTP API server, MCP mode, and A2A protocol support.</p>

  </div>
  <div class="feature">
    <div class="feature-icon">🔒</div>
    <h3>Security-First Design</h3>
    <p>Tool confirmation prompts, containerized MCP tools via Docker, client isolation, and resource scoping.</p>

  </div>
</div>

## Why docker-agent?

After spending years building AI agents using various frameworks, the Docker team kept asking the same questions:

- **How do we make building agents less of a hassle?** — Most agents use the same building blocks. docker-agent provides them out of the box.
- **Can we reuse those building blocks?** — Declarative YAML or HCL configs mean you can mix and match agents, models, and tools without rewriting code.
- **How can we share agents easily?** — Push agents to any OCI registry and run them anywhere with a single command.

docker-agent is built in the open so the community can make use of this work and contribute to its future.

## How it Works

At its core, docker-agent follows a simple loop:

1. **You define agents** in YAML or HCL — their model, instructions, tools, and sub-agents.
2. **You run an agent** via the TUI, CLI, or API.
3. **The agent processes your request** — calling tools, delegating to sub-agents, and reasoning step by step.
4. **Results stream back in real-time** via an event-driven architecture.

```yaml
# A minimal agent definition
agents:
  root:
    model: openai/gpt-5-mini
    description: A helpful assistant
    instruction: You are a helpful assistant.
    toolsets:
      - type: think
```

```bash
# Run it
$ docker agent run agent.yaml
```

<div class="callout callout-tip" markdown="1">
<div class="callout-title">💡 Tip
</div>
  <p>Jump straight to the <a href="{{ '/getting-started/quickstart/' | relative_url }}">Quick Start</a> if you want to build your first agent right away.</p>

</div>

## What's Next?

<div class="cards">
  <a class="card" href="{{ '/getting-started/installation/' | relative_url }}">
    <div class="card-icon">📥</div>
    <h3>Installation</h3>
    <p>Install docker-agent on macOS, Linux, or Windows.</p>
  </a>
  <a class="card" href="{{ '/getting-started/quickstart/' | relative_url }}">
    <div class="card-icon">⚡</div>
    <h3>Quick Start</h3>
    <p>Build your first agent in under 5 minutes.</p>
  </a>
</div>
