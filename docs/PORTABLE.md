# Portable Deployment

This document explains how to deploy `codex-mimo-adapter` on a Windows x64 machine
without a Rust toolchain, using the pre-built portable ZIP package.

## Launcher runtime updates

The portable ZIP remains supported. The desktop launcher distribution uses a separate
Windows x64 core runtime instead: `codex-mimo-adapter.exe` plus
`runtime/windows-x64/manifest.json`. The manifest records the adapter version,
SHA-256, platform, and minimum compatible launcher version.

Development machines and CI create that runtime with:

```powershell
.\scripts\stage-runtime-windows.ps1 -MinimumLauncherVersion "0.1.0"
```

The executable and generated manifest are deliberately ignored by Git. Do not commit
them or API keys to the repository. CI uploads both a core-only runtime artifact and
a combined end-user artifact. The combined artifact has this layout:

```text
CodexMiMoLauncher.exe
runtime/windows-x64/codex-mimo-adapter.exe
runtime/windows-x64/manifest.json
```

The current launcher requires this matching runtime below the same extracted root;
it does not download a runtime for a bare source clone. The manifest is the integrity
contract for a runtime package.

## Choose the right installation path

- **Development machine:** clone the source repository and install Rust to build the
  core runtime. Building the self-contained Windows launcher also requires the .NET
  8 SDK. Use `stage-runtime-windows.ps1` to place a local core in the expected
  runtime directory.
- **Normal Windows user:** download and extract the CI combined Windows x64 artifact
  (or a future combined Release package), then run `CodexMiMoLauncher.exe` from its
  extracted root. Do not treat a bare `git clone` or the core-only runtime artifact
  as a ready-to-run installation.

The launcher saves an entered MiMo API key outside the repository using the current
Windows user's DPAPI profile. On first run it invokes
`codex-mimo-adapter init --api-key-stdin`: the key is delivered through standard
input, project configuration records only that the key comes from the process
environment, and the subsequently launched core inherits that environment value.

## Package Contents

```
codex-mimo-adapter-{version}-windows-x64/
  codex-mimo-adapter.exe    # The adapter binary
  .env.example              # Environment variable template
  config.toml.example       # Codex provider configuration template
  agents/                   # Global subagent templates (route rewritten on first start)
  scripts/
    start-portable.ps1      # One-command startup script
    check-local-adapter.ps1 # Health check script
  PORTABLE.md               # This file
  README.md                 # Project README
  NOTICE                    # Third-party notices
```

## Prerequisites

- Windows 10/11 x64
- PowerShell 5.1+ (ships with Windows)
- A MiMo Token Plan API key

## Quick Start

1. **Extract** the ZIP archive to any directory.

2. **Configure** the adapter by editing `.env` (or copying from `.env.example`):
   ```powershell
   # Set your API key
   MIMO_API_KEY=your-mimo-token-plan-api-key
   ```

3. **Start** the adapter:
   ```powershell
   .\scripts\start-portable.ps1 -ApiKey "your-mimo-token-plan-api-key"
   ```

   On subsequent runs, use the same command without `-ApiKey`; the script
   reuses the saved project key:
   ```powershell
   .\scripts\start-portable.ps1
   ```

   Each run checks the project `.codex/agents` directory. If it finds a
   legacy four-agent installation or a missing managed agent, it reruns `init`
   once to migrate to the current nine-agent set while preserving whether the
   existing key was project-stored or process-inherited. `-NoInit` explicitly
   disables both first-time initialization and this migration check.

   The first run also installs the routed agent definitions into
   `%USERPROFILE%\.codex\agents`, so they are available to every Codex project
   for the current Windows user. It writes the package EXE's absolute path as
   the provider authentication helper, so no standalone CLI or PATH change is
   required. Restart Codex Desktop after the first run.

   To configure the target machine without starting the service immediately:
   ```powershell
   .\scripts\start-portable.ps1 -ApiKey "your-mimo-token-plan-api-key" -ConfigureOnly
   ```

4. **Verify** it is working:
   ```powershell
   .\scripts\check-local-adapter.ps1
   ```

## Configuration

### Environment Variables (.env)

| Variable | Default | Description |
|---|---|---|
| `MIMO_API_KEY` | (required) | Your MiMo Token Plan API key |
| `MIMO_API_BASE_URL` | `https://token-plan-cn.xiaomimimo.com/v1` | Upstream API base URL |
| `CODEX_MIMO_LOCAL_TOKEN` | `codex-mimo-local` | Local auth token for the adapter |
| `CODEX_MIMO_PROJECT_ID` | `mimo_adapter_example` | Project identifier |
| `CODEX_MIMO_HOST` | `127.0.0.1` | Listen address |
| `CODEX_MIMO_PORT` | `4010` | Listen port |
| `CODEX_MIMO_STATE_DB` | `.codex-mimo/state.sqlite` | State database path |

### Codex Provider Configuration (config.toml.example)

Merge the contents of `config.toml.example` into your user-level Codex config:

- **Windows**: `%USERPROFILE%\.codex\config.toml`
- **macOS/Linux**: `~/.codex/config.toml`

This registers the `mimo_adapter` provider for Codex to use.

## Integration with Codex

After starting the adapter, configure Codex to route through it:

```powershell
# Add the mimo_adapter provider to your global Codex config
codex-mimo-adapter init --api-key "your-key"
```

Or manually add the provider block from `config.toml.example` to
`%USERPROFILE%\.codex\config.toml`. The startup script is still recommended
because it also installs globally routed subagent definitions.

## Troubleshooting

### Adapter won't start

- Ensure no other process is using port 4010.
- Check that `.env` contains a valid `MIMO_API_KEY`.

### Health check fails

- Confirm the adapter is running in another terminal.
- Verify the port matches (default: 4010).

### Models list is empty

- Run `codex-mimo-adapter init` to set up the project registry.

## Uninstalling

Simply delete the extracted directory. The adapter is self-contained and does
not write to the Windows registry or Program Files.
