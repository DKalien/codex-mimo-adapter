[CmdletBinding()]
param(
    [string]$ApiKey = "",
    [string]$BaseUrl = "https://token-plan-cn.xiaomimimo.com/v1",
    [string]$ListenHost = "127.0.0.1",
    [int]$Port = 4010,
    [string]$LocalToken = "codex-mimo-local",
    [string]$StateDb = ".codex-mimo/state.sqlite",
    [int]$MaxConcurrency = 8,
    [switch]$Release
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot

if ([string]::IsNullOrWhiteSpace($ApiKey)) {
    $ApiKey = $env:MIMO_API_KEY
}

if ([string]::IsNullOrWhiteSpace($ApiKey)) {
    throw "MIMO_API_KEY is required. Pass -ApiKey or set the environment variable first."
}

if (-not [System.IO.Path]::IsPathRooted($StateDb)) {
    $StateDb = Join-Path $repoRoot $StateDb
}

$existing = Get-NetTCPConnection -State Listen -LocalPort $Port -ErrorAction SilentlyContinue |
    Select-Object -First 1
if ($existing) {
    Write-Host "Stopping existing process on port $Port (PID $($existing.OwningProcess))"
    Stop-Process -Id $existing.OwningProcess -Force
    Start-Sleep -Milliseconds 500
}

$env:MIMO_API_KEY = $ApiKey
$env:MIMO_API_BASE_URL = $BaseUrl
$env:CODEX_MIMO_HOST = $ListenHost
$env:CODEX_MIMO_PORT = "$Port"
$env:CODEX_MIMO_LOCAL_TOKEN = $LocalToken
$env:CODEX_MIMO_STATE_DB = $StateDb
$env:CODEX_MIMO_MAX_CONCURRENCY = "$MaxConcurrency"

Write-Host "Starting adapter from repo:"
Write-Host " - repo root: $repoRoot"
Write-Host " - base URL:  $BaseUrl"
Write-Host " - listen:    http://${ListenHost}:$Port"
Write-Host " - state DB:  $StateDb"
Write-Host " - concurrency: $MaxConcurrency"

Push-Location $repoRoot
try {
    if ($Release) {
        cargo run --release
    } else {
        cargo run
    }
} finally {
    Pop-Location
}
