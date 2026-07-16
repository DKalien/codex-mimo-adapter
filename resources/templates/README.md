# resources/templates — Canonical managed agent templates

This directory holds the canonical copy of every managed subagent configuration template shipped with codex-mimo-adapter.

## Purpose

- The Rust binary `include_str!()`s (or otherwise embeds) files from this directory to produce default agent configs during `codex-mimo-adapter init`.
- `.codex/agents/*.toml` at the project root are the runtime/user-facing copies; this directory is the source of truth.
- Keep these files in sync with `.codex/agents/` when updating agent models or instructions.

## Files

| File | Agent | Model | Reasoning effort |
|---|---|---|---|
| default.toml | default | mimo/mimo-v2.5 | high |
| explorer.toml | explorer | mimo/mimo-v2.5 | high |
| oss-worker-pro-1.toml | oss_worker_pro_analysis | mimo/mimo-v2.5-pro | high |
| oss-worker-pro-2.toml | oss_worker_pro_implementation | mimo/mimo-v2.5-pro | high |
| oss-worker-pro-3.toml | oss_worker_pro_review | mimo/mimo-v2.5-pro | high |
| oss-worker-std-1.toml | oss_worker_std_implementation | mimo/mimo-v2.5 | high |
| oss-worker-std-2.toml | oss_worker_std_test | mimo/mimo-v2.5 | high |
| oss-worker-std-3.toml | oss_worker_std_docs | mimo/mimo-v2.5 | high |
| worker.toml | worker | mimo/mimo-v2.5 | high |

## Usage from Rust

```rust
// Embed a template at compile time:
pub const DEFAULT_TOML: &str = include_str!("templates/default.toml");
```

All files use UTF-8 encoding.
