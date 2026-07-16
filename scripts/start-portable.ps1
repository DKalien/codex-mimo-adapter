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
    Skip the init step, including automatic migration of legacy agent files.

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

$ManagedAgentFiles = @(
    "default.toml",
    "explorer.toml",
    "oss-worker-pro-1.toml",
    "oss-worker-pro-2.toml",
    "oss-worker-pro-3.toml",
    "oss-worker-std-1.toml",
    "oss-worker-std-2.toml",
    "oss-worker-std-3.toml",
    "worker.toml"
)
$LegacyManagedAgentFiles = @(
    "oss-flash.toml",
    "oss-mimo.toml",
    "oss-minimax.toml",
    "oss-pro.toml"
)

function Test-ManagedProjectAgentsCurrent {
    param([string]$AgentsDirectory)

    if (-not (Test-Path -LiteralPath $AgentsDirectory -PathType Container)) {
        return $false
    }
    foreach ($agentFile in $ManagedAgentFiles) {
        if (-not (Test-Path -LiteralPath (Join-Path $AgentsDirectory $agentFile) -PathType Leaf)) {
            return $false
        }
    }
    foreach ($legacyFile in $LegacyManagedAgentFiles) {
        if (Test-Path -LiteralPath (Join-Path $AgentsDirectory $legacyFile) -PathType Leaf) {
            return $false
        }
    }
    return $true
}

function Get-ProjectEnvironmentSettings {
    param([string]$Path)

    $settings = @{}
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        return $settings
    }
    Get-Content -LiteralPath $Path | ForEach-Object {
        if ($_ -notmatch '=') { return }
        $key, $value = $_ -split '=', 2
        $settings[$key.Trim()] = $value.Trim()
    }
    return $settings
}

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

# ── Run init for first setup or managed-agent migration ──────────────
$projectEnvPath = Join-Path $packageDir ".codex-mimo-adapter.env"
$projectAgentsDir = Join-Path $packageDir ".codex\agents"
$projectAlreadyInitialized = Test-Path -LiteralPath $projectEnvPath -PathType Leaf
$managedAgentsCurrent = Test-ManagedProjectAgentsCurrent -AgentsDirectory $projectAgentsDir
$shouldRunInit = (-not $NoInit) -and ((-not $projectAlreadyInitialized) -or (-not $managedAgentsCurrent))

if ($shouldRunInit) {
    $existingProjectSettings = Get-ProjectEnvironmentSettings -Path $projectEnvPath
    $usesProcessApiKey = $existingProjectSettings["CODEX_MIMO_API_KEY_SOURCE"] -eq "process"
    if ($usesProcessApiKey) {
        if ([string]::IsNullOrWhiteSpace($ApiKey)) {
            $ApiKey = $envVars["MIMO_API_KEY"]
        }
        if ([string]::IsNullOrWhiteSpace($ApiKey) -or $ApiKey -eq "your-mimo-token-plan-api-key") {
            throw "MIMO_API_KEY is required to migrate this process-inherited API key configuration. Pass -ApiKey or set MIMO_API_KEY in .env."
        }
    } else {
        if ([string]::IsNullOrWhiteSpace($ApiKey)) {
            $ApiKey = $existingProjectSettings["MIMO_API_KEY"]
        }
        if ([string]::IsNullOrWhiteSpace($ApiKey)) {
            $ApiKey = $envVars["MIMO_API_KEY"]
        }
        if ([string]::IsNullOrWhiteSpace($ApiKey) -or $ApiKey -eq "your-mimo-token-plan-api-key") {
            throw "API key is required on first run. Pass -ApiKey or set MIMO_API_KEY in .env."
        }
    }

    if ($projectAlreadyInitialized) {
        Write-Host "Migrating managed agent definitions ..."
    } else {
        Write-Host "Initializing project ..."
    }
    Push-Location $packageDir
    try {
        if ($usesProcessApiKey) {
            $ApiKey | & $exePath init --api-key-stdin --host $ListenHost --port $Port --upstream-base $UpstreamBase
        } else {
            & $exePath init --api-key $ApiKey --host $ListenHost --port $Port --upstream-base $UpstreamBase
        }
        if ($LASTEXITCODE -ne 0) {
            throw "init failed with exit code $LASTEXITCODE."
        }
    } finally {
        Pop-Location
    }
} else {
    Write-Host "Project and managed agent definitions are current (or -NoInit specified). Skipping init."
}

if (-not (Test-Path -LiteralPath $projectEnvPath)) {
    throw "Project configuration was not created at $projectEnvPath. Run again without -NoInit."
}

# ── Install globally routed agent definitions ────────────────────────
$projectSettings = Get-ProjectEnvironmentSettings -Path $projectEnvPath
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
foreach ($agentFile in $ManagedAgentFiles) {
    $sourcePath = Join-Path $agentTemplateDir $agentFile
    if (-not (Test-Path -LiteralPath $sourcePath -PathType Leaf)) {
        throw "Managed agent template is missing: $sourcePath"
    }
    $content = Get-Content -LiteralPath $sourcePath -Raw
    $routed = [regex]::Replace(
        $content,
        'mimo_adapter/[0-9a-fA-F]{12}/mimo/',
        "mimo_adapter/$projectKey/mimo/"
    )
    if ($routed -eq $content -and $content -match 'model_provider\s*=\s*"mimo_adapter"') {
        throw "Could not rewrite the project route in $sourcePath."
    }
    $targetPath = Join-Path $globalAgentDir $agentFile
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
