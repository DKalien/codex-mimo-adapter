# Codex MiMo Launcher

Windows WinForms launcher for the portable adapter runtime. It deliberately keeps the GUI separate from the Rust core:

- The runtime is discovered at `runtime/windows-x64/`. Its `manifest.json` must have schema version `1`, platform `windows-x64`, a valid adapter file/SHA-256, and a compatible `minimum_launcher_version`. A missing manifest is shown as “runtime not installed”; an invalid manifest prevents startup.
- API keys are protected with the current Windows user's DPAPI at `%LOCALAPPDATA%\\CodexMiMoLauncher\\mimo-api-key.dpapi`; they are never passed on the command line or written below the repository by this launcher.
- Start initializes a first-use repository through `init --api-key-stdin`, then starts `run` with `MIMO_API_KEY` set only in the child process environment. On upgrade it also performs this initialization once when any of the nine managed agent templates is missing or when a legacy `oss-flash`/`oss-mimo`/`oss-minimax`/`oss-pro` template is present; a complete nine-template installation is not reinitialized.
- After successful initialization it updates only `[model_providers.mimo_adapter.auth] command` in `%USERPROFILE%\\.codex\\config.toml` to the resolved absolute core-EXE path (and writes a timestamped `launcher.bak` backup). This lets Codex request a local token without a separately installed CLI on `PATH`.
- The launcher only stops the process it started. A healthy process already using port 4010 is reported as external and never killed.
- Only one launcher window runs per Windows user session. Opening the EXE again restores the existing window instead of starting another service. Choosing “退出启动器” from the tray first stops the adapter process started by that launcher, then exits; an external adapter is never stopped.

Development build:

```powershell
dotnet build launcher/CodexMiMoLauncher.csproj
```

Portable release build:

```powershell
dotnet publish launcher/CodexMiMoLauncher.csproj -c Release -r win-x64 --self-contained true -p:PublishSingleFile=true
```

The required manifest is `runtime/windows-x64/manifest.json`; the window shows `adapter.version` after checksum validation. Runtime updates therefore do not require rebuilding the launcher, but must always include a matching manifest.
