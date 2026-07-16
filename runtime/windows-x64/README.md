# Windows x64 Runtime

This directory is the stable launcher's runtime contract. At runtime it must contain:

```text
runtime/windows-x64/
  codex-mimo-adapter.exe
  manifest.json
```

`manifest.json` is generated beside the executable and contains its SHA-256, adapter
version, target platform, and `minimum_launcher_version`. A launcher must verify the
SHA-256 before starting the executable and reject a runtime that requires a newer
launcher version.

The executable and generated manifest are intentionally not committed to Git. CI
builds a core-only runtime artifact and a combined end-user artifact that places this
directory beside `CodexMiMoLauncher.exe`. This keeps source clones small and prevents
binary history from accumulating in the repository. The current launcher does not
download the runtime itself.

## Build or stage on a development machine

```powershell
.\scripts\stage-runtime-windows.ps1 -MinimumLauncherVersion "0.1.0"
```

The script builds `x86_64-pc-windows-msvc` by default, copies the release executable
to this directory, and writes `manifest.json`. For safety, output is limited to this
runtime directory or the repository's `dist` directory. To stage a binary already
built by CI:

```powershell
.\scripts\stage-runtime-windows.ps1 -SkipBuild -OutputDirectory dist\runtime\windows-x64
```

Only the development machine or CI needs Rust. End users need the runtime artifact,
the launcher, and a MiMo API key; they do not need a Rust toolchain.
