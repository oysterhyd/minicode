$ErrorActionPreference = 'Stop'
$nativeRoot = Split-Path $PSScriptRoot -Parent
Push-Location $nativeRoot
try {
    # MyGo 0.5 covers what the old vendor patches added (OnTextKey for the
    # composer, paint-only poses and drawings, frame pacing), so the vendor
    # tree is upstream as published.
    go mod tidy
    if ($LASTEXITCODE -ne 0) { throw 'go mod tidy failed' }
    go mod vendor
    if ($LASTEXITCODE -ne 0) { throw 'go mod vendor failed' }
} finally { Pop-Location }
