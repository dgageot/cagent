---
title: "Google Gemini"
description: "Use Gemini 2.5 Flash, Gemini 3.1 Pro, and other Google models with Docker Agent."
keywords: docker agent, ai agents, model providers, llm, google gemini
weight: 120
canonical: https://docs.docker.com/ai/docker-agent/providers/google/
---

_Use Gemini 2.5 Flash, Gemini 3.1 Pro, and other Google models with Docker Agent._

## Setup

Docker Agent reads the first credential it finds from these environment variables (see `pkg/model/provider/gemini/client.go`):

| Variable                    | Purpose                                                                             |
| --------------------------- | ----------------------------------------------------------------------------------- |
| `GOOGLE_API_KEY`            | Primary Gemini API key.                                                             |
| `GEMINI_API_KEY`            | Alternative name for the Gemini API key (also used by the official Google SDK).     |
| `GOOGLE_GENAI_USE_VERTEXAI` | When set (any value), routes through Vertex AI instead of the Gemini Developer API. |
| `GOOGLE_CLOUD_PROJECT`      | GCP project used when `GOOGLE_GENAI_USE_VERTEXAI` is set or for Vertex AI Model Garden. |
| `GOOGLE_CLOUD_LOCATION`     | GCP region for Vertex AI (defaults to the SDK default).                             |

On the Gemini Developer API, a model or [custom provider](../custom/index.md) that sets `token_key` reads its key from that variable instead of `GOOGLE_API_KEY` / `GEMINI_API_KEY`. The Vertex AI backends use Application Default Credentials and ignore `token_key`.

```bash
# Gemini Developer API
export GOOGLE_API_KEY="AI..."   # or GEMINI_API_KEY

# Vertex AI (no API key; uses Application Default Credentials)
gcloud auth application-default login
export GOOGLE_GENAI_USE_VERTEXAI=1
export GOOGLE_CLOUD_PROJECT="my-gcp-project"
export GOOGLE_CLOUD_LOCATION="us-central1"
```

## Configuration

### Inline

```yaml
agents:
  root:
    model: google/gemini-3.8-flash
```

### Named Model

```yaml
models:
  gemini:
    provider: google
    model: gemini-3.8-flash
    temperature: 0.5
```

## Available Models

| Model                     | Best For                        |
| -------------------------- | ------------------------------- |
| `gemini-3.1-pro-preview`  | Most capable Gemini model       |
| `gemini-3.8-flash`        | Fast, efficient, good balance   |
| `gemini-2.5-flash`        | Fast inference, cost-effective  |
| `gemini-2.5-pro`          | Strong reasoning, large context |

## Generated Images

Some Gemini models (e.g. `gemini-2.5-flash-image`) are designed to generate
an image directly as part of their reply, not just describe one. Docker
Agent requests that image output on supported Google surfaces — the models
gateway, direct Gemini API, and Vertex AI — according to one binding policy:
explicit `false`, explicit `true`, then an exact models.dev record whose
`Modalities.Output` contains `image`. An omitted image flag, including
`output_capabilities: {}`, uses that catalogue default. Unknown models or
unavailable catalogue data leave image response modalities disabled; model
names are never used to guess capability.
Each eligible ordinary chat request asks for text *and* image output.
Vertex AI has deterministic guard/predicate coverage; live image-generation
validation is deferred.

Docker Agent attempts to save each returned image and display it in the TUI — see [Generated Media](../../features/tui/index.md#generated-media)
for file naming, collision handling, and rendering details.

The workspace file is the visible deliverable. After that file and its manifest
entry are saved, a portable copy is also stored in the session database and
preferred when the session is reopened. It can render the original generated
bytes even if the workspace file was edited, moved, or deleted.
Generated-media storage and resolution do not impose a size cap. If portable
persistence fails, the workspace file is still kept and the turn includes a
warning.
Sessions created before portable copies were introduced continue to use their
manifest-gated workspace files.

```yaml
models:
  gemini-image:
    provider: google
    model: gemini-2.5-flash-image
```

When `output_capabilities.image` is omitted, including in an empty block,
Docker Agent uses models.dev output modalities for the exact known model. Set
it explicitly for custom models or to override incorrect catalogue data;
unknown or unavailable metadata remains disabled and capability is never
guessed from the model name.

Session-title and compaction requests omit image response modalities and
bypass the guard even for image-output-capable models. They do not explicitly
force TEXT-only output. Ordinary image-output requests with custom function tools or
structured output are rejected locally before any request is sent. Google
Search, Maps, and code-execution built-ins remain available. When custom
tools conflict, Docker Agent uses models.dev's `tool_call` capability to clarify
whether the model cannot call tools at all or supports tools only outside an
image-output request. Unknown catalogue data keeps the conservative generic
message.

A few provider-side behaviors to know:

- **The provider decides the image format** (typically PNG). Asking for a
  `.gif` or `.svg` filename does not transcode anything — the saved file's
  extension is corrected to match the data actually returned.
- **An image is not guaranteed.** Even a correctly configured image model
  can answer with text only and generate no image. At a text-only stop,
  phrases such as "generate an image" or "draw a picture" in the last user
  prompt trigger a nonfatal warning while preserving the reply:
  `The model returned text but no image for this image-generation request. Try rephrasing the request.`
  This phrase-based check is not semantic intent detection and does not
  check output capability: negated or quoted phrases can match and other
  wording can be missed. It does not track a whole submission across tool
  calls, steering, stop hooks, or handoffs. A terminal provider error skips
  this check, as does structured output configured on the current agent
  model; per-call overrides and reply content are not independently
  classified.

## Thinking Budget

Gemini supports two approaches depending on the model version:

> [!WARNING]
> **Different thinking formats**
>
> Gemini 2.5 uses **token-based** budgets (integers). Gemini 3 uses **level-based** budgets (strings like `low`, `high`). Make sure you use the right format for your model version.

### Gemini 2.5 (Token-based)

```yaml
models:
  gemini-no-thinking:
    provider: google
    model: gemini-2.5-flash
    thinking_budget: 0 # disable thinking

  gemini-dynamic:
    provider: google
    model: gemini-2.5-flash
    thinking_budget: -1 # dynamic (model decides) — default

  gemini-fixed:
    provider: google
    model: gemini-2.5-flash
    thinking_budget: 8192 # fixed token budget
```

### Gemini 3 (Level-based)

```yaml
models:
  gemini-pro:
    provider: google
    model: gemini-3.1-pro-preview
    thinking_budget: high # default for Pro: low | high

  gemini-flash:
    provider: google
    model: gemini-3.8-flash
    thinking_budget: medium # default for Flash: minimal | low | medium | high
```

## Built-in Tools (Grounding)

Gemini models support built-in tools that let the model access Google Search and Google Maps
directly during generation. Enable them via `provider_opts`:

```yaml
models:
  gemini-grounded:
    provider: google
    model: gemini-2.5-flash
    provider_opts:
      google_search: true
      google_maps: true
      code_execution: true
```

| Option           | Description                                          |
| ---------------- | ---------------------------------------------------- |
| `google_search`  | Enables Google Search grounding for up-to-date info  |
| `google_maps`    | Enables Google Maps grounding for location queries   |
| `code_execution` | Enables server-side code execution for computations  |

### Seeing what the model did

Built-in tools run on Google's side, so they never go through the local tool
loop: no confirmation prompt, no tool result in the conversation, and nothing
is re-executed when the session is replayed. Their activity is still made
visible:

- **Search queries** (`google_search`) and **code execution** (`code_execution`,
  with the code, its language and the captured output or error) appear as
  completed tool cards in the TUI and are printed in `--exec` runs.
- **URL fetches** made by the `url_context` tool are listed per URL with their
  retrieval status; paywalled, unsafe or failed fetches show as errors.
- When built-in tools are combined with your own tools, Gemini also returns
  the server-side invocations themselves (`toolCall`/`toolResponse` parts).
  Those are shown with their arguments and response under the tool's name
  (`google_search`, `google_maps`, `url_context`, ...) and, once seen,
  suppress the query and URL summaries above. A summary that streamed before
  the first explicit invocation is still shown, so an invocation can
  occasionally appear twice.
- **Sources** the answer was grounded on (grounding chunks and quoted
  citations) are listed as a `Sources:` footer under the response, one entry
  per URL.

Both are persisted on the assistant message (`citations`,
`server_tool_calls`) so they show up again when a session is reopened, and are
streamed to API clients as `server_tool_call` and `agent_citations` events
(see the [API server](../../features/api-server/index.md)). The assistant text
itself is left untouched — structured output and JSON schema responses are
not affected. Two gaps: a turn that produced only built-in tool activity and
no text, local tool call or media is not recorded, so its annotations are
lost when the session is reopened; and the Markdown transcript export does
not include citations or server tool calls.

In `--exec` runs the cards and the `Sources:` footer are written to stdout
along with the answer. Pass `--hide-tool-calls` when piping the output to
another program, or use `--json` for one event per line.

Known limitation: server-side `toolCall`/`toolResponse` parts are not echoed
back to Gemini on later turns (the SDK recommends doing so when built-in and
function tools are mixed). This predates their display and is not addressed
by it: the model does not see its own earlier built-in invocations, which may
affect follow-up turns that rely on them.

## Vertex AI Model Garden

You can use non-Gemini models (e.g. Claude, Llama) hosted on Google Cloud's
[Vertex AI Model Garden](https://cloud.google.com/vertex-ai/generative-ai/docs/partner-models/use-partner-models)
through the `google` provider. When a `publisher` is specified in `provider_opts`,
requests are routed through the appropriate Vertex AI endpoint instead of the
Gemini SDK:

- **Anthropic Claude** (`publisher: anthropic`) uses the Anthropic-native
  `:rawPredict` / `:streamRawPredict` endpoints. Claude models on Vertex AI do
  not support the OpenAI `/chat/completions` path.
- **Other publishers** (e.g. `meta`, `mistral`) use Vertex AI's
  OpenAI-compatible `/chat/completions` endpoint.

### Authentication

Vertex AI uses Google Cloud Application Default Credentials (ADC). Make sure you
are authenticated:

```bash
gcloud auth application-default login
```

### Configuration

```yaml
models:
  claude-on-vertex:
    provider: google
    model: claude-sonnet-5
    provider_opts:
      project: my-gcp-project       # GCP project ID (or set GOOGLE_CLOUD_PROJECT)
      location: us-east5             # GCP region (or set GOOGLE_CLOUD_LOCATION)
      publisher: anthropic           # Model publisher (anthropic, meta, etc.)
```

| Option      | Description                                                                          |
| ----------- | ------------------------------------------------------------------------------------ |
| `project`   | GCP project ID. Falls back to `GOOGLE_CLOUD_PROJECT` env var                         |
| `location`  | GCP region (e.g. `us-east5`, `us-central1`). Falls back to `GOOGLE_CLOUD_LOCATION`   |
| `publisher`  | Model publisher (e.g. `anthropic`, `meta`, `mistral`). Must not be `google`          |

> [!NOTE]
> **Gemini models on Vertex AI**
>
> Setting `publisher: google` (or omitting `publisher`) uses the native Gemini SDK path. The Model Garden endpoint is only used for non-Google publishers.
