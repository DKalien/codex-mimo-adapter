# Codex MiMo Token Plan Adapter

A deliberately thin adapter between the OpenAI Responses API used by Codex subagents and the MiMo Token Plan Chat Completions API.

It converts protocol shapes; it does not run another agent system.

## Data flow

```text
Codex subagent
  -> POST /v1/responses (Responses API)
  -> this adapter
  -> POST /chat/completions (Chat Completions API)
  -> MiMo Token Plan (https://token-plan-cn.xiaomimimo.com/v1)
  -> Chat Completions response / SSE stream
  -> this adapter
  -> Responses API response / SSE stream
  -> Codex subagent
```

Codex remains responsible for task roles, sandbox permissions, tool execution, review, and deciding whether work is complete.

## Install

This project is not published to `crates.io` yet.

Install the current local checkout as a global Cargo CLI:

```powershell
cd D:\AI-Tools\codex-mimo-adapter
cargo install --path .
```

If you update the local source and want to reinstall the current checkout:

```powershell
cargo install --path . --force
```

After installation, `codex-mimo-adapter` is available as a global command.

## Quick Start

Initialize a project, start the adapter, and verify it works:

```powershell
# 1. Initialize the current project with your MiMo Token Plan API key
codex-mimo-adapter init --api-key "<your-mimo-token-plan-api-key>"
#   - Writes .codex-mimo-adapter.env (project-level env)
#   - Registers the project in the global registry (~/.codex-mimo-adapter/)
#   - Writes .codex/agents/*.toml with routed model names
#   - Writes ~/.codex/config.toml with a single "mimo_adapter" provider

# 2. Start the adapter (single instance serving all registered projects)
codex-mimo-adapter run

# 3. In another terminal, verify the adapter is healthy
codex-mimo-adapter check

# 4. Print the signed local token (used by the Codex provider auth command)
codex-mimo-adapter auth print-local-token
```

During development you can still run from source:

```powershell
cargo run -- init --api-key "<your-key>"
cargo run -- run
```

Or use the repo-local helper:

```powershell
./scripts/dev-run.ps1 -ApiKey "<your-key>"
```

### Agent templates and model routing

`init` writes OSS subagent templates into `.codex/agents/` with a routed model format:

| Field | Value | Example |
|---|---|---|
| `model_provider` | `mimo_adapter` (fixed) | `mimo_adapter` |
| `model` | `mimo_adapter/<project_key>/<real_model>` | `mimo_adapter/c8b0cfc9ca15/mimo/deepseek-v4-flash` |

The project key is a short hash derived from the project root path. The adapter server parses this format to extract the project and upstream model, then routes the request to the correct API key and upstream base URL.

The old bare format `mimo/<model>` is no longer supported; run `init` again to regenerate templates.

### Multi-project usage

You can initialize multiple project directories - each gets its own `.codex-mimo-adapter.env`, agent templates with its own project key, and a separate registry entry. The single `codex-mimo-adapter run` instance loads all registered projects on startup and routes requests by project key.

To load a newly initialized project without restarting the adapter, call:

```powershell
curl.exe -X POST http://127.0.0.1:4010/admin/refresh -H "Authorization: Bearer $(codex-mimo-adapter auth print-local-token)"
```

The `/admin/refresh` endpoint reads the registry and loads any projects not already in memory.

### Configuration

`init` writes the default runtime settings into the current project's `.codex-mimo-adapter.env`, including a `CODEX_MIMO_PROJECT_ID`. Edit that file when you need to change the stored API key, port, token, or SQLite path.

Each project directory has its own `.codex-mimo-adapter.env`. The adapter also maintains a global registry at `~/.codex-mimo-adapter/project-registry.toml` to discover projects at startup.

Runtime precedence is `CLI flags > .codex-mimo-adapter.env > process environment > defaults`.
For available variables see the [Environment variables](#environment-variables) table below.
`run`/`start` loads all registered projects from the global registry. `check` reads config from the closest project env file.
`auth print-local-token` finds a local token from the closest project env or any registered project, then signs an adapter-level token.
Project routing is handled entirely by the adapter server via `model = mimo_adapter/<project_key>/<real_model>`.
### Sanity check

```powershell
codex-mimo-adapter check
```

This command verifies that the local adapter is running (`/health`) and the models endpoint (`/v1/models`) responds with a valid token.

`./scripts/check-local-adapter.ps1` remains available as a legacy helper, but the CLI command is the primary path.

Full smoke suite when needed:

```powershell
./scripts/run-real-smoke.ps1 -ApiKey "<your-key>"
```

If you only need one doc, start with [docs/USAGE.zh-CN.md](docs/USAGE.zh-CN.md). Real validation results are in [docs/REAL_VALIDATION_2026-06-25.zh-CN.md](docs/REAL_VALIDATION_2026-06-25.zh-CN.md).

## Environment variables

| Variable | Default | Description |
|---|---|---|
| `MIMO_API_KEY` | required | API key for MiMo Token Plan. |
| `CODEX_MIMO_LOCAL_TOKEN` | generated by `init` | Bearer token required from local callers. If empty, auth is skipped. |
| `CODEX_MIMO_PROJECT_ID` | generated by `init` | Project identifier stored in `.codex-mimo-adapter.env`. |
| `CODEX_MIMO_HOST` | `127.0.0.1` | Listen host. |
| `CODEX_MIMO_PORT` | `4010` | Listen port. |
| `MIMO_API_BASE_URL` | `https://token-plan-cn.xiaomimimo.com/v1` | MiMo Token Plan upstream base URL. |
| `CODEX_MIMO_STATE_DB` | `.codex-mimo/state.sqlite` | SQLite state database path (relative to project root). |
| `CODEX_MIMO_STATE_TTL_SECONDS` | `21600` | State TTL, 6 hours. |
| `CODEX_MIMO_TIMEOUT_SECONDS` | `300` | Upstream request timeout. |
| `CODEX_MIMO_MAX_REQUEST_BYTES` | `8388608` | Max request body size, 8 MB. |
| `CODEX_MIMO_MAX_CONCURRENCY` | `8` | Maximum concurrent upstream requests, read at startup. |
| `RUST_LOG` | `codex_mimo_adapter=info` | Tracing filter. Use `codex_mimo_adapter=debug` for detailed diagnostics. |

The upstream API key and local client token must be different. The adapter never logs either token.

If you see `adapter concurrency limit reached`, check the current project's `.codex-mimo-adapter.env` first.
That message means the adapter's own `CODEX_MIMO_MAX_CONCURRENCY` limit was exhausted or configured too low; it is not, by itself, evidence that the upstream model vendor only supports one request at a time.

## Endpoints

| Method | Path | Description |
|---|---|---|
| `POST` | `/v1/responses` | Responses API, streaming and non-streaming. |
| `GET` | `/v1/models` | List available models. Model IDs use the `mimo_adapter/<project_key>/mimo/<id>` prefix. Requires adapter bearer token. |
| `GET` | `/health` | Health check. |
| `POST` | `/admin/refresh` | Hot-reload projects from the registry without restarting the adapter. Requires adapter bearer token. |

## Tests

```bash
cargo test
```

Real upstream smoke:

```bash
MIMO_API_KEY="your-key" cargo test --test e2e_real_smoke test_e2e_real_validation_suite -- --ignored --nocapture
```

## Current status

The adapter is usable for self-hosted Codex subagent routing with MiMo Token Plan. Text, stream, tool call, custom tool, tool search, continuation, and multimodal guard paths have mock coverage, and real MiMo Token Plan smoke validation has been run against the current setup.

See [docs/ROADMAP.md](docs/ROADMAP.md) for status and next milestones.

## More Docs

| File | Purpose |
|---|---|
| [docs/USAGE.zh-CN.md](docs/USAGE.zh-CN.md) | Short self-use setup and troubleshooting guide. |
| [docs/REAL_VALIDATION_2026-06-25.zh-CN.md](docs/REAL_VALIDATION_2026-06-25.zh-CN.md) | Latest real upstream smoke and partial Codex validation record. |
| [docs/VALIDATION.zh-CN.md](docs/VALIDATION.zh-CN.md) | Full manual validation checklist. |
| [docs/DIAGNOSTICS.md](docs/DIAGNOSTICS.md) | Runtime diagnostics and log interpretation. |
| [scripts/install-user-provider.ps1](scripts/install-user-provider.ps1) | Legacy wrapper that now points to `codex-mimo-adapter init`. |
| [scripts/check-local-adapter.ps1](scripts/check-local-adapter.ps1) | Legacy PowerShell helper for `/health` and `/v1/models`. |
| [docs/COMPATIBILITY.md](docs/COMPATIBILITY.md) | Compatibility scope and non-goals. |
| [docs/ROADMAP.md](docs/ROADMAP.md) | Current status and future ideas. |

## Explicit non-goals

Not planned:

- full cc-switch port
- provider aggregation platform
- UI, hooks, plugins, statusLine, or upstream session management
- automatic model fallback/routing
- automatic multimodal retry after stripping media
- silent multimodal degradation that makes a text-only model pretend it saw media

See [docs/COMPATIBILITY.md](docs/COMPATIBILITY.md) for the full compatibility scope.
