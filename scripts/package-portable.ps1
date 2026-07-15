<#
.SYNOPSIS
    Build and package a portable Windows x64 distribution of codex-mimo-adapter.

.DESCRIPTION
    Runs on the development machine. Builds a release binary, collects it with
    non-secret configuration templates and deployment scripts, validates that
    no secrets are included, and produces a versioned ZIP archive.

    Prerequisites:
      - Rust toolchain with the x86_64-pc-windows-msvc target installed
      - cargo on PATH
      - PowerShell 5.1+ (ships with Windows)

.PARAMETER Version
    Override the version string extracted from Cargo.toml.
    Defaults to the version field in the [package] section.

.PARAMETER OutputDir
    Directory for the final ZIP archive.
    Defaults to .\dist

.PARAMETER TestSecretScan
    Run the secret scanner's positive, negative, and binary-ignore self-tests
    without building or packaging the adapter.

.EXAMPLE
    .\scripts\package-portable.ps1
    .\scripts\package-portable.ps1 -Version 0.2.0 -OutputDir C:\out
    .\scripts\package-portable.ps1 -TestSecretScan
#>
[CmdletBinding()]
param(
    [string]$Version,
    [string]$OutputDir,
    [switch]$TestSecretScan
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Test-PortableTextFile {
    param([System.IO.FileInfo]$File)

    $textExtensions = @(
        ".cfg", ".conf", ".env", ".example", ".json", ".md", ".ps1",
        ".toml", ".txt", ".xml", ".yaml", ".yml"
    )
    $extension = $File.Extension.ToLowerInvariant()
    $name = $File.Name.ToLowerInvariant()

    return ($textExtensions -contains $extension) -or
        ($name -eq "license") -or
        ($name -eq "notice") -or
        ($name -eq "readme")
}

function Test-SecretPlaceholder {
    param(
        [string]$Value,
        [switch]$AllowLocalDefault
    )

    $normalized = $Value.Trim()
    if ([string]::IsNullOrWhiteSpace($normalized)) {
        return $true
    }
    if ($AllowLocalDefault -and $normalized -eq "codex-mimo-local") {
        return $true
    }

    return $normalized -match '^(?i)(your(?:[-_].*)?|example|placeholder|changeme|replace(?:[-_].*)?|<[^>]+>|\$\{[^}]+\}|\{\{[^}]+\}\})$'
}

function Get-EnvironmentAssignmentValues {
    param(
        [string]$Content,
        [string]$VariableName
    )

    $escapedName = [regex]::Escape($VariableName)
    $pattern = '(?im)(?:^|[\s;])(?:export\s+)?' + $escapedName +
        '\s*=\s*(?:"([^"\r\n]*)"|''([^''\r\n]*)''|([^\s;#\r\n]+))'
    foreach ($match in [regex]::Matches($Content, $pattern)) {
        if ($match.Groups[1].Success) {
            $match.Groups[1].Value
        } elseif ($match.Groups[2].Success) {
            $match.Groups[2].Value
        } else {
            $match.Groups[3].Value
        }
    }
}

function Find-SecretMatchesInText {
    param([string]$Content)

    foreach ($value in @(Get-EnvironmentAssignmentValues -Content $Content -VariableName "MIMO_API_KEY")) {
        if (-not (Test-SecretPlaceholder -Value $value)) {
            [pscustomobject]@{ Kind = "MIMO_API_KEY assignment" }
        }
    }
    foreach ($value in @(Get-EnvironmentAssignmentValues -Content $Content -VariableName "CODEX_MIMO_LOCAL_TOKEN")) {
        if (-not (Test-SecretPlaceholder -Value $value -AllowLocalDefault)) {
            [pscustomobject]@{ Kind = "CODEX_MIMO_LOCAL_TOKEN assignment" }
        }
    }

    $opaquePatterns = @(
        @{ Kind = "Bearer token"; Pattern = '(?i)Bearer\s+[A-Za-z0-9_-]{20,}' },
        @{ Kind = "sk token"; Pattern = '(?i)sk-[A-Za-z0-9_-]{20,}' },
        @{ Kind = "local adapter token"; Pattern = '(?i)codex-mimo-[0-9a-f]{16,}' }
    )
    foreach ($entry in $opaquePatterns) {
        if ($Content -match $entry.Pattern) {
            [pscustomobject]@{ Kind = $entry.Kind }
        }
    }
}

function Invoke-PortableSecretScan {
    param([string]$RootPath)

    foreach ($file in Get-ChildItem -LiteralPath $RootPath -Recurse -File) {
        if (-not (Test-PortableTextFile -File $file)) {
            continue
        }

        $content = Get-Content -LiteralPath $file.FullName -Raw
        foreach ($finding in @(Find-SecretMatchesInText -Content $content)) {
            [pscustomobject]@{
                File = $file.FullName
                Kind = $finding.Kind
            }
        }
    }
}

function Invoke-SecretScanSelfTest {
    $testRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("codex-mimo-secret-scan-" + [guid]::NewGuid().ToString("N"))
    $negativeRoot = Join-Path $testRoot "negative"
    $positiveRoot = Join-Path $testRoot "positive"
    New-Item -ItemType Directory -Path $negativeRoot, $positiveRoot -Force | Out-Null

    try {
        $negativeContent = @'
MIMO_API_KEY=your-mimo-token-plan-api-key
MIMO_API_KEY="your-key" cargo test
CODEX_MIMO_LOCAL_TOKEN=codex-mimo-local
Authorization: Bearer $LocalToken
Authorization: Bearer $(codex-mimo-adapter auth print-local-token)
'@
        Set-Content -LiteralPath (Join-Path $negativeRoot ".env.example") -Value $negativeContent -Encoding UTF8

        # A binary containing secret-like variable names must never be decoded as text.
        $binaryBytes = [System.Text.Encoding]::UTF8.GetBytes(
            "MIMO_API_KEY=fake_binary_value Bearer AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
        )
        [System.IO.File]::WriteAllBytes((Join-Path $negativeRoot "fixture.exe"), $binaryBytes)

        $positiveContent = @'
MIMO_API_KEY=mimo_live_ABCDEFGHIJKLMNOPQRSTUVWXYZ123456
CODEX_MIMO_LOCAL_TOKEN=codex-mimo-0123456789abcdef0123456789abcdef
Authorization: Bearer BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB
OPENAI_API_KEY=sk-CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC
'@
        Set-Content -LiteralPath (Join-Path $positiveRoot "leaked.env") -Value $positiveContent -Encoding UTF8

        $negativeFindings = @(Invoke-PortableSecretScan -RootPath $negativeRoot)
        if ($negativeFindings.Count -ne 0) {
            throw "Secret scan self-test failed: placeholders or binary content produced a false positive."
        }

        $positiveFindings = @(Invoke-PortableSecretScan -RootPath $positiveRoot)
        $expectedKinds = @(
            "MIMO_API_KEY assignment",
            "CODEX_MIMO_LOCAL_TOKEN assignment",
            "Bearer token",
            "sk token",
            "local adapter token"
        )
        foreach ($kind in $expectedKinds) {
            if ($positiveFindings.Kind -notcontains $kind) {
                throw "Secret scan self-test failed: did not detect $kind."
            }
        }

        Write-Host "Secret scan self-test passed (placeholders allowed, binary ignored, synthetic secrets detected)."
    } finally {
        if (Test-Path -LiteralPath $testRoot) {
            Remove-Item -LiteralPath $testRoot -Recurse -Force
        }
    }
}

if ($TestSecretScan) {
    Invoke-SecretScanSelfTest
    return
}

# ── Resolve paths ──────────────────────────────────────────────────────
$repoRoot = Split-Path -Parent $PSScriptRoot

if ([string]::IsNullOrWhiteSpace($OutputDir)) {
    $OutputDir = Join-Path $repoRoot "dist"
}

# ── Read version from Cargo.toml ──────────────────────────────────────
if ([string]::IsNullOrWhiteSpace($Version)) {
    $cargoToml = Get-Content (Join-Path $repoRoot "Cargo.toml") -Raw
    if ($cargoToml -match '(?m)^version\s*=\s*"([^"]+)"') {
        $Version = $Matches[1]
    } else {
        throw "Could not extract version from Cargo.toml. Pass -Version explicitly."
    }
}

Write-Host "Package version : $Version"

# ── Prerequisite checks ───────────────────────────────────────────────
$cargo = Get-Command "cargo" -ErrorAction SilentlyContinue
if (-not $cargo) {
    throw "cargo is not on PATH. Install the Rust toolchain first: https://rustup.rs"
}

$target = "x86_64-pc-windows-msvc"
$installed = & rustup target list --installed 2>$null
if ($installed -notcontains $target) {
    Write-Host "Installing Rust target $target ..."
    & rustup target add $target
    if ($LASTEXITCODE -ne 0) {
        throw "Failed to install Rust target $target."
    }
}

# ── Build release binary ──────────────────────────────────────────────
Write-Host "Building release binary (target: $target) ..."
Push-Location $repoRoot
try {
    & cargo build --release --target $target
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build failed with exit code $LASTEXITCODE."
    }
} finally {
    Pop-Location
}

$exePath = Join-Path $repoRoot "target\$target\release\codex-mimo-adapter.exe"
if (-not (Test-Path -LiteralPath $exePath)) {
    throw "Release binary not found at $exePath"
}

$exeSize = (Get-Item -LiteralPath $exePath).Length
Write-Host "Binary size      : $([math]::Round($exeSize / 1MB, 1)) MB"

# ── Prepare staging directory ─────────────────────────────────────────
$archiveName = "codex-mimo-adapter-$Version-windows-x64"
$stagingDir  = Join-Path $OutputDir $archiveName
$zipPath     = Join-Path "$OutputDir" "$archiveName.zip"

if (Test-Path -LiteralPath $stagingDir) {
    Remove-Item -LiteralPath $stagingDir -Recurse -Force
}
if (Test-Path -LiteralPath $zipPath) {
    Remove-Item -LiteralPath $zipPath -Force
}
New-Item -ItemType Directory -Path $stagingDir -Force | Out-Null
New-Item -ItemType Directory -Path (Join-Path $stagingDir "scripts") -Force | Out-Null
New-Item -ItemType Directory -Path (Join-Path $stagingDir "agents") -Force | Out-Null

# ── Copy files into staging ───────────────────────────────────────────
# Binary
Copy-Item -LiteralPath $exePath -Destination (Join-Path $stagingDir "codex-mimo-adapter.exe")

# Non-secret configuration templates
Copy-Item -LiteralPath (Join-Path $repoRoot ".env.example")        -Destination (Join-Path $stagingDir ".env.example")
Copy-Item -LiteralPath (Join-Path $repoRoot "config.toml.example") -Destination (Join-Path $stagingDir "config.toml.example")

# Global agent templates.  start-portable.ps1 rewrites the project route for
# the target machine before installing these under the user's Codex home.
Get-ChildItem -LiteralPath (Join-Path $repoRoot ".codex\agents") -Filter "*.toml" -File | ForEach-Object {
    Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $stagingDir "agents\$($_.Name)")
}

# Deployment scripts for the target machine
Copy-Item -LiteralPath (Join-Path $repoRoot "scripts\start-portable.ps1")       -Destination (Join-Path $stagingDir "scripts\start-portable.ps1")
Copy-Item -LiteralPath (Join-Path $repoRoot "scripts\check-local-adapter.ps1")  -Destination (Join-Path $stagingDir "scripts\check-local-adapter.ps1")

# Documentation
Copy-Item -LiteralPath (Join-Path $repoRoot "docs\PORTABLE.md") -Destination (Join-Path $stagingDir "PORTABLE.md")
Copy-Item -LiteralPath (Join-Path $repoRoot "README.md")        -Destination (Join-Path $stagingDir "README.md")
Copy-Item -LiteralPath (Join-Path $repoRoot "NOTICE")           -Destination (Join-Path $stagingDir "NOTICE")

# ── Secret scan ───────────────────────────────────────────────────────
# Scan text candidates only. Known documentation/template placeholders are
# allowed, but non-placeholder assignments and opaque token forms still fail.
$secretFindings = @(Invoke-PortableSecretScan -RootPath $stagingDir)
foreach ($finding in $secretFindings) {
    Write-Warning "SECRET PATTERN matched in $($finding.File): $($finding.Kind)"
}
if ($secretFindings.Count -gt 0) {
    # Clean up staging before failing
    Remove-Item -LiteralPath $stagingDir -Recurse -Force
    throw "Secret scan failed. The staging directory was removed. Fix the source templates and retry."
}

# ── Create ZIP ────────────────────────────────────────────────────────
Write-Host "Creating $zipPath ..."
Compress-Archive -Path (Join-Path $stagingDir "*") -DestinationPath $zipPath -Force

# Verify the ZIP was created
if (-not (Test-Path -LiteralPath $zipPath)) {
    throw "Failed to create ZIP archive at $zipPath"
}

$zipSize = (Get-Item -LiteralPath $zipPath).Length
Write-Host ""
Write-Host "=== Portable package ready ==="
Write-Host "Archive  : $zipPath"
Write-Host "Size     : $([math]::Round($zipSize / 1MB, 1)) MB"
Write-Host "Version  : $Version"
Write-Host ""
Write-Host "Contents :"
Get-ChildItem -Path $stagingDir -Recurse -File | ForEach-Object {
    $relative = $_.FullName.Substring($stagingDir.Length + 1)
    Write-Host "  $relative"
}

# ── Clean up staging (keep the ZIP) ──────────────────────────────────
Remove-Item -LiteralPath $stagingDir -Recurse -Force
Write-Host ""
Write-Host "Staging directory removed. ZIP is ready for distribution."
