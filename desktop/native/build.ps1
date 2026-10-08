$ErrorActionPreference = 'Stop'
Push-Location $PSScriptRoot
try {
    $output = Join-Path $PSScriptRoot '../../output/native'
    New-Item -ItemType Directory -Force $output | Out-Null
    $env:CGO_ENABLED = '0'
    go build -trimpath -ldflags '-s -w -H=windowsgui' -o (Join-Path $output 'MiniCode.exe') .
    if ($LASTEXITCODE -ne 0) { throw 'Native Desktop build failed' }
} finally { Pop-Location }
