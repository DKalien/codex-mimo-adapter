<#
.SYNOPSIS
    Builds and stages the Windows x64 adapter runtime for a launcher or CI artifact.

.DESCRIPTION
    Produces only the core adapter executable and a machine-readable manifest. The
    output is intentionally ignored by Git: release binaries must be distributed as
    CI artifacts or release assets, never committed with source code.

.PARAMETER MinimumLauncherVersion
    The oldest launcher version allowed to run this core runtime.

.PARAMETER OutputDirectory
    Runtime directory to create. Relative paths are resolved from the repository root.

.PARAMETER SkipBuild
    Reuse an existing release executable. Intended for CI after its cargo build step
    and for local manifest verification.
#>
[CmdletBinding()]
param(
    [string]$MinimumLauncherVersion = "0.1.0",
    [string]$OutputDirectory = "runtime\windows-x64",
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Resolve-RepositoryPath {
    param([Parameter(Mandatory = $true)][string]$Path)

    if ([System.IO.Path]::IsPathRooted($Path)) {
        return [System.IO.Path]::GetFullPath($Path)
    }
    return [System.IO.Path]::GetFullPath((Join-Path $repoRoot $Path))
}

function Test-PathWithin {
    param(
        [Parameter(Mandatory = $true)][string]$Candidate,
        [Parameter(Mandatory = $true)][string]$Parent
    )

    $normalizedCandidate = $Candidate.TrimEnd([System.IO.Path]::DirectorySeparatorChar, [System.IO.Path]::AltDirectorySeparatorChar)
    $normalizedParent = $Parent.TrimEnd([System.IO.Path]::DirectorySeparatorChar, [System.IO.Path]::AltDirectorySeparatorChar)
    if ($normalizedCandidate.Equals($normalizedParent, [System.StringComparison]::OrdinalIgnoreCase)) {
        return $true
    }
    $prefix = $normalizedParent + [System.IO.Path]::DirectorySeparatorChar
    return $normalizedCandidate.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)
}

$repoRoot = Split-Path -Parent $PSScriptRoot
$repoRoot = [System.IO.Path]::GetFullPath($repoRoot)
$target = "x86_64-pc-windows-msvc"
$sourceExe = Join-Path $repoRoot "target\$target\release\codex-mimo-adapter.exe"
$outputPath = Resolve-RepositoryPath -Path $OutputDirectory
$runtimeRoot = [System.IO.Path]::GetFullPath((Join-Path $repoRoot "runtime\windows-x64"))
$distRoot = [System.IO.Path]::GetFullPath((Join-Path $repoRoot "dist"))

if (-not (Test-PathWithin -Candidate $outputPath -Parent $runtimeRoot) -and
    -not (Test-PathWithin -Candidate $outputPath -Parent $distRoot)) {
    throw "OutputDirectory must be inside $runtimeRoot or $distRoot."
}
if ($MinimumLauncherVersion -notmatch '^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$') {
    throw "MinimumLauncherVersion must be a semantic version, for example 0.1.0."
}

if (-not $SkipBuild) {
    $cargo = Get-Command cargo -ErrorAction SilentlyContinue
    if ($null -eq $cargo) {
        throw "cargo is not on PATH. Build the runtime on a Rust development machine or in CI."
    }

    Push-Location $repoRoot
    try {
        & cargo build --release --target $target
        if ($LASTEXITCODE -ne 0) {
            throw "cargo build failed with exit code $LASTEXITCODE."
        }
    } finally {
        Pop-Location
    }
}

if (-not (Test-Path -LiteralPath $sourceExe -PathType Leaf)) {
    throw "Release executable not found at $sourceExe. Run without -SkipBuild after installing the Windows MSVC target."
}

$cargoTomlPath = Join-Path $repoRoot "Cargo.toml"
$cargoToml = Get-Content -LiteralPath $cargoTomlPath -Raw
if ($cargoToml -notmatch '(?m)^version\s*=\s*"([^"]+)"') {
    throw "Could not read package version from $cargoTomlPath."
}
$adapterVersion = $Matches[1]

New-Item -ItemType Directory -Path $outputPath -Force | Out-Null
$stagedExe = Join-Path $outputPath "codex-mimo-adapter.exe"
$manifestPath = Join-Path $outputPath "manifest.json"
Copy-Item -LiteralPath $sourceExe -Destination $stagedExe -Force
$sha256 = (Get-FileHash -LiteralPath $stagedExe -Algorithm SHA256).Hash.ToLowerInvariant()

$manifest = [ordered]@{
    schema_version = 1
    platform = "windows-x64"
    adapter = [ordered]@{
        name = "codex-mimo-adapter"
        version = $adapterVersion
        file = "codex-mimo-adapter.exe"
        sha256 = $sha256
    }
    minimum_launcher_version = $MinimumLauncherVersion
    generated_at_utc = [DateTime]::UtcNow.ToString("o")
}
$json = $manifest | ConvertTo-Json -Depth 4
[System.IO.File]::WriteAllText(
    $manifestPath,
    ($json + [Environment]::NewLine),
    (New-Object System.Text.UTF8Encoding($false))
)

Write-Host "Runtime staged successfully"
Write-Host "Directory : $outputPath"
Write-Host "Adapter   : $adapterVersion"
Write-Host "SHA-256   : $sha256"
Write-Host "Manifest  : $manifestPath"
