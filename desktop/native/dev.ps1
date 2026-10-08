$ErrorActionPreference = 'Stop'
Push-Location $PSScriptRoot
try {
    go run .
    if ($LASTEXITCODE -ne 0) { throw 'Native Desktop exited with an error' }
} finally { Pop-Location }
