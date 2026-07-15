<#
.SYNOPSIS
    Initialize and start the codex-mimo-adapter from a portable package.

.DESCRIPTION
    Designed to run on the target deployment machine after extracting the
    portable ZIP.  Loads .env, runs `codex-mimo-adapter init` if needed,
    and starts the adapter process.

.PARAMETER ApiKey
    MiMo Token Plan API key.  Required on first run; subsequent runs
    read the key from the existing .codex-mimo-adapter.env.

.PARAMETER ListenHost
    Bind address.  Defaults to 127.0.0.1.

.PARAMETER Port
    Listen port.  Defaults to 4010.

.PARAMETER UpstreamBase
    MiMo Token Plan base URL.
    Defaults to https://token-plan-cn.xiaomimimo.com/v1

.PARAMETER NoInit
    Skip the init step (useful when .env already exists).

.PARAMETER ConfigureOnly
    Initialize the adapter and install global Codex configuration, then exit
    without starting the long-running adapter process.

.EXAMPLE
    .\scripts\start-portable.ps1 -ApiKey "your-mimo-api-key"
    .\scripts\start-portable.ps1 -NoInit
#>
[CmdletBinding()]
param(
    [string]$ApiKey,
    [string]$ListenHost = "127.0.0.1",
    [int]$Port = 4010,
    [string]$UpstreamBase = "https://token-plan-cn.xiaomimimo.com/v1",
    [switch]$NoInit,
    [switch]$ConfigureOnly
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$packageDir = Split-Path -Parent $PSScriptRoot
$exePath    = Join-Path $packageDir "codex-mimo-adapter.exe"
$envPath    = Join-Path $packageDir ".env"
$envExample = Join-Path $packageDir ".env.example"

# ── Check binary exists ───────────────────────────────────────────────
if (-not (Test-Path -LiteralPath $exePath)) {
    throw "codex-mimo-adapter.exe not found in $packageDir. Ensure the portable package is complete."
}

# ── Ensure .env exists ────────────────────────────────────────────────
if (-not (Test-Path -LiteralPath $envPath)) {
    if (Test-Path -LiteralPath $envExample) {
        Write-Host "Creating .env from .env.example ..."
        Copy-Item -LiteralPath $envExample -Destination $envPath
    } else {
        throw ".env not found and .env.example is missing. Cannot proceed."
    }
}

# ── Load environment variables from .env ─────────────────────────────
Write-Host "Loading environment from .env ..."
$envVars = @{}
Get-Content -LiteralPath $envPath | ForEach-Object {
    $line = $_.Trim()
    if ($line -match '^\s*#' -or $line -notmatch '=') { return }
    $key, $value = $line -split '=', 2
    $envVars[$key.Trim()] = $value.Trim()
}
foreach ($kv in $envVars.GetEnumerator()) {
    [Environment]::SetEnvironmentVariable($kv.Key, $kv.Value, "Process")
}

# ── Run init if not already initialized ──────────────────────────────
$projectEnvPath = Join-Path $packageDir ".codex-mimo-adapter.env"
if ((-not $NoInit) -and (-not (Test-Path -LiteralPath $projectEnvPath))) {
    if ([string]::IsNullOrWhiteSpace($ApiKey)) {
        # Try to read from .env
        $ApiKey = $envVars["MIMO_API_KEY"]
    }
    if ([string]::IsNullOrWhiteSpace($ApiKey) -or $ApiKey -eq "your-mimo-token-plan-api-key") {
        throw "API key is required on first run. Pass -ApiKey or set MIMO_API_KEY in .env."
    }
    Write-Host "Initializing project ..."
    Push-Location $packageDir
    try {
        & $exePath init --api-key $ApiKey --host $ListenHost --port $Port --upstream-base $UpstreamBase
        if ($LASTEXITCODE -ne 0) {
            throw "init failed with exit code $LASTEXITCODE."
        }
    } finally {
        Pop-Location
    }
} else {
    Write-Host "Project already initialized (or -NoInit specified). Skipping init."
}

if (-not (Test-Path -LiteralPath $projectEnvPath)) {
    throw "Project configuration was not created at $projectEnvPath. Run again without -NoInit."
}

# ── Install globally routed agent definitions ────────────────────────
$projectSettings = @{}
Get-Content -LiteralPath $projectEnvPath | ForEach-Object {
    if ($_ -notmatch '=') { return }
    $key, $value = $_ -split '=', 2
    $projectSettings[$key.Trim()] = $value.Trim()
}
$projectId = $projectSettings["CODEX_MIMO_PROJECT_ID"]
if ([string]::IsNullOrWhiteSpace($projectId)) {
    throw "CODEX_MIMO_PROJECT_ID is missing from $projectEnvPath."
}
$projectKey = $projectId -replace '^mimo_adapter_', ''
$agentTemplateDir = Join-Path $packageDir "agents"
$userProfile = $env:USERPROFILE
if ([string]::IsNullOrWhiteSpace($userProfile)) {
    $userProfile = [Environment]::GetFolderPath("UserProfile")
}
$globalConfigPath = Join-Path $userProfile ".codex\config.toml"
$globalAgentDir = Join-Path $userProfile ".codex\agents"

if (-not (Test-Path -LiteralPath $agentTemplateDir)) {
    throw "Global agent templates are missing from $agentTemplateDir."
}
if (-not (Test-Path -LiteralPath $globalConfigPath)) {
    throw "Codex global configuration was not created at $globalConfigPath."
}
if ($exePath.Contains("'")) {
    throw "Portable path contains a single quote and cannot be written safely to Codex config: $exePath"
}

# The target machine does not need the standalone CLI on PATH. Point the
# provider's auth helper directly at the executable inside this package.
$globalConfig = Get-Content -LiteralPath $globalConfigPath -Raw
$authCommandPattern = '(?m)(^\[model_providers\.mimo_adapter\.auth\]\s*\r?\n\s*command\s*=\s*)(?:"[^"]*"|''[^'']*'')'
$authCommandReplacement = '${1}' + "'$exePath'"
$updatedGlobalConfig = [regex]::Replace(
    $globalConfig,
    $authCommandPattern,
    $authCommandReplacement,
    1
)
if ($updatedGlobalConfig -eq $globalConfig) {
    throw "Could not update the mimo_adapter auth command in $globalConfigPath."
}
[System.IO.File]::WriteAllText($globalConfigPath, $updatedGlobalConfig, (New-Object System.Text.UTF8Encoding($false)))

New-Item -ItemType Directory -Path $globalAgentDir -Force | Out-Null
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
Get-ChildItem -LiteralPath $agentTemplateDir -Filter "*.toml" -File | ForEach-Object {
    $content = Get-Content -LiteralPath $_.FullName -Raw
    $routed = [regex]::Replace(
        $content,
        'mimo_adapter/[0-9a-fA-F]{12}/mimo/',
        "mimo_adapter/$projectKey/mimo/"
    )
    if ($routed -eq $content -and $content -match 'model_provider\s*=\s*"mimo_adapter"') {
        throw "Could not rewrite the project route in $($_.FullName)."
    }
    $targetPath = Join-Path $globalAgentDir $_.Name
    [System.IO.File]::WriteAllText($targetPath, $routed, $utf8NoBom)
}
Write-Host "Installed globally routed Codex agents in $globalAgentDir"

if ($ConfigureOnly) {
    Write-Host "Portable configuration completed; adapter start skipped because -ConfigureOnly was specified."
    return
}

# ── Start the adapter ────────────────────────────────────────────────
Write-Host ""
Write-Host "Starting codex-mimo-adapter on ${ListenHost}:${Port} ..."
Write-Host "Press Ctrl+C to stop."
Write-Host ""

Push-Location $packageDir
try {
    & $exePath run --host $ListenHost --port $Port --upstream-base $UpstreamBase
} finally {
    Pop-Location
}
