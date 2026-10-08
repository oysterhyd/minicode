param([string]$Installation = (Join-Path $PSScriptRoot 'dist/native/windows-amd64'))
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$installationPath = [IO.Path]::GetFullPath($Installation)
$manifestPath = Join-Path $installationPath 'update-windows-amd64.json'
$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
$archivePath = Join-Path $installationPath ([IO.Path]::GetFileName(([uri]$manifest.url).AbsolutePath))
$configPath = Join-Path $root 'desktop/native/mygo.json'
$python = Join-Path $installationPath 'runtime/python/python.exe'
& $python -I -X utf8 (Join-Path $PSScriptRoot 'verify-update.py') $manifestPath $configPath
if ($LASTEXITCODE -ne 0) { throw 'Update signature verification failed' }
$qaTarget = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot ('.cache/update-qa-本地 升级-' + [guid]::NewGuid().ToString('N'))))
$qaCache = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '.cache'))
if (-not $qaTarget.StartsWith($qaCache+[IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw 'Invalid update QA directory' }
New-Item -ItemType Directory -Force $qaTarget | Out-Null
& $python -I -X utf8 -c 'import sys,tarfile; tarfile.open(sys.argv[1]).extractall(sys.argv[2],filter="data")' $archivePath $qaTarget
if ($LASTEXITCODE -ne 0) { throw 'Update archive extraction failed' }
$listener = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
$listener.Start()
$qaPort = $listener.LocalEndpoint.Port
$listener.Stop()
$qaKey = (Get-Content -LiteralPath $configPath -Raw | ConvertFrom-Json).updates.publicKey
$ldflags = "-X github.com/egoist/mygo.production=1 -X github.com/egoist/mygo.packageUpdateFeed=http://127.0.0.1:$qaPort/update.json -X github.com/egoist/mygo.packageUpdateKey=$qaKey"
Push-Location (Join-Path $root 'desktop/native')
try {
    go build -trimpath -ldflags $ldflags -o (Join-Path $qaTarget 'MiniCode.exe') ./tools/updatecheck
    if ($LASTEXITCODE -ne 0) { throw 'Update SDK probe build failed' }
} finally { Pop-Location }
& (Join-Path $qaTarget 'MiniCode.exe') -manifest $manifestPath -archive $archivePath -target $qaTarget -expected (Join-Path $installationPath 'MiniCode.exe') -port $qaPort
if ($LASTEXITCODE -ne 0) { throw 'Update SDK install/tamper probe failed' }
& (Join-Path $PSScriptRoot 'smoke-native.ps1') -Installation $qaTarget
if ($LASTEXITCODE -ne 0) { throw 'Updated installation smoke failed' }
Write-Host "Signed update, tamper rejection and updated runtime verified: $qaTarget"
