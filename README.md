# Codex OpenCode Go Adapter

A deliberately thin adapter between the OpenAI Responses API used by Codex
subagents and the OpenCode Go Chat Completions API.

中文安装、配置与低 Token 排障说明见
[docs/USAGE.zh-CN.md](docs/USAGE.zh-CN.md)。

It converts protocol shapes; it does not run another agent system. Mission
tiers, semantic gates, automatic patch application, answer grading, OpenCode
sessions, and OpenCode tools are intentionally absent.

## Data flow

```text
Codex subagent
  -> POST /v1/responses
  -> this adapter
  -> POST https://opencode.ai/zen/go/v1/chat/completions
  -> Responses JSON or SSE
```

Codex remains responsible for task roles, sandbox permissions, tool execution,
review, and deciding whether work is complete.

## Run

```powershell
$env:OPENCODE_GO_API_KEY = "..."
$env:CODEX_OPENCODE_LOCAL_TOKEN = "use-a-different-local-token"
$env:CODEX_OPENCODE_PORT = "4010"
python -m codex_opencode_adapter
```

The upstream API key and local client token must be different. The adapter
never logs either token. Copy the provider and agent examples from
`config.toml.example` and `.codex/agents/` into the desired Codex configuration.

The provider block must be installed in the user-level Codex config. Codex
ignores `model_providers` declared in a project's `.codex/config.toml`.

## Reasoning compatibility

The adapter reads `reasoning.effort` or `reasoning_effort` from a Responses
request. It sends `reasoning_effort` upstream only when model metadata declares
the requested variant using the verified OpenAI-compatible protocol.

Current verified profiles:

- DeepSeek V4 Pro/Flash: `low`, `medium`, `high`, `max`
- MiMo V2.5/Pro: `low`, `medium`, `high`

Models that support reasoning but do not declare adjustable variants keep their
default behavior. Unsupported settings are reported in structured adapter
metadata and logs rather than silently pretending to work.

Reasoning content is retained only in stored chat history so tool continuations
remain valid. It is not exposed as user-visible chain of thought.

## Endpoints

- `POST /v1/responses`
- `GET /v1/models`
- `GET /health`

State needed for `previous_response_id` and tool results is stored in a local
SQLite database and expires according to `CODEX_OPENCODE_STATE_TTL_SECONDS`.
