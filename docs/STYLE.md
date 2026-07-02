# Documentation style guide

A short reference for writers and reviewers. Goal: keep voice, naming
and examples consistent across every page on this site.

## Product naming

| Context | Use | Don't use |
|---|---|---|
| Prose, headings, marketing | **Docker Agent** (two words, both capitalised — the proper name of the product) | docker-agent, Docker-Agent, docker agent (in prose) |
| The CLI command | `docker agent` (lower-case, two words, in monospace) | `docker-agent`, `Docker Agent run` |
| The repository / module path | `docker/docker-agent` | docker/Docker-Agent |
| Internal identifiers / package names | as defined in code (e.g. `cagent`) — never invent new spellings in prose | mixing internal identifiers into user-facing copy |

A simple rule of thumb:

- **Talking about the product?** → "Docker Agent"
- **Showing a command the user types?** → `docker agent run agent.yaml`

## Voice

- Address the reader as **you**, not "we" or "the user".
- Prefer present tense and active voice ("the agent reads files",
  not "files will be read by the agent").
- Keep sentences short. Two short sentences usually beat one compound
  one.
- Avoid "simply", "just", "easily" — they're rarely accurate and
  often condescending.

## Code samples

- All shell prompts use `$` followed by a space, with the command on the
  same line. Output, when shown, has no prompt.
- YAML/HCL examples should be runnable as-is when reasonable, or end
  in `# ...` to make truncation explicit.
- The canonical example agent uses `model: anthropic/claude-sonnet-4-5`.
  Use a different model only when the example is *about* that model.
- File names in prose are in `monospace` (`agent.yaml`, not "agent.yaml").

## Callouts

Use GitHub-style alerts — they render natively on docs.docker.com and
are upgraded to the styled callout panels on github.io:

```markdown
> [!TIP]
> **When to use it**: Body text.
```

- `[!NOTE]` — neutral context
- `[!TIP]` — positive, "consider this"
- `[!WARNING]` — caution, breaking, security

An optional bold lead-in (`**Title**:`) names the callout; skip it when
the kind alone says enough. Don't prefix it with an emoji — the icon
badge already provides one.

## Links

Use plain relative Markdown links between pages, including the
`index.md` filename, so both renderers resolve them. From a page two
levels deep (e.g. `concepts/agents/index.md`):

```markdown
See [Models](../../concepts/models/index.md) for details.
```

Never use Liquid (`relative_url`) or root-absolute paths — they break
when the docs are mounted on docs.docker.com.

## Glossary one-liners

When a page first introduces a term, link to its concept page or use
one of these standard one-liners:

- **Agent** — an LLM with instructions, tools, and (optionally)
  sub-agents, defined in YAML or HCL.
- **Toolset** — a group of related tools the agent can call (e.g.
  `filesystem`, `shell`, `mcp`).
- **MCP** — Model Context Protocol, an open standard for tool servers.
- **A2A** — Agent-to-Agent protocol, used to talk to other agents
  over HTTP.
- **TUI** — Terminal User Interface, the default interactive front end
  Docker Agent ships with.
- **OCI** — Open Container Initiative; the same registry format used
  for Docker images. Docker Agent reuses it for sharing agents.
