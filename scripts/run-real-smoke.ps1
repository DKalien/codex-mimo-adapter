param(
    [string]$BaseUrl = "https://token-plan-cn.xiaomimimo.com/v1",
    [string]$TextModel = "mimo/deepseek-v4-flash",
    [string]$VisionModel = "mimo/mimo-v2.5",
    [string]$ApiKey = ""
)

$ErrorActionPreference = "Stop"

if ([string]::IsNullOrWhiteSpace($ApiKey)) {
    $ApiKey = $env:MIMO_API_KEY
}

if ([string]::IsNullOrWhiteSpace($ApiKey)) {
    throw "MIMO_API_KEY is required. Pass -ApiKey or set the environment variable first."
}

$env:MIMO_API_KEY = $ApiKey
$env:MIMO_API_BASE_URL = $BaseUrl
$env:MIMO_API_REAL_TEXT_MODEL = $TextModel
$env:MIMO_API_REAL_VISION_MODEL = $VisionModel

Write-Host "Running real smoke suite against $BaseUrl"
Write-Host "Text model: $TextModel"
Write-Host "Vision model: $VisionModel"

cargo test --test e2e_real_smoke test_e2e_real_validation_suite -- --ignored --nocapture
