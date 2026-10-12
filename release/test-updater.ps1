param([string]$Installation = (Join-Path $PSScriptRoot 'dist/native/windows-amd64'))
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$installationPath = [IO.Path]::GetFullPath($Installation)
$manifestPath = Join-Path $installationPath 'update-windows-amd64.json'
$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
$archivePath = Join-Path $installationPath ([IO.Path]::GetFileName(([uri]$manifest.url).AbsolutePath))
$configPath = Join-Path $root 'desktop/native/mygo.json'
$node = Join-Path $installationPath 'runtime/node/node.exe'
& $node (Join-Path $PSScriptRoot 'verify-update.mjs') $manifestPath $configPath
if ($LASTEXITCODE -ne 0) { throw 'Update signature verification failed' }
$qaTarget = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot ('.cache/update-qa-本地 升级-' + [guid]::NewGuid().ToString('N'))))
$qaCache = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '.cache'))
if (-not $qaTarget.StartsWith($qaCache+[IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw 'Invalid update QA directory' }
New-Item -ItemType Directory -Force $qaTarget | Out-Null
# Windows tar.exe can pass Unicode paths through the runner's ANSI code page.
# The .NET extractor preserves the same Chinese/space path used by the SDK probe.
$archiveStream = [IO.File]::OpenRead($archivePath)
$gzipStream = [IO.Compression.GZipStream]::new($archiveStream, [IO.Compression.CompressionMode]::Decompress)
try { [System.Formats.Tar.TarFile]::ExtractToDirectory($gzipStream, $qaTarget, $false) }
finally { $gzipStream.Dispose(); $archiveStream.Dispose() }
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
$liveStart = [Diagnostics.ProcessStartInfo]::new((Join-Path $qaTarget 'runtime/minicode.exe'))
$liveStart.UseShellExecute = $false
$liveStart.CreateNoWindow = $true
$liveStart.RedirectStandardInput = $true
$liveStart.RedirectStandardOutput = $true
$liveStart.ArgumentList.Add('bridge')
$liveStart.Environment['MINICODE_DB_PATH'] = Join-Path $qaTarget 'live-session.db'
$liveRuntime = [Diagnostics.Process]::Start($liveStart)
try {
    $liveRuntime.StandardInput.WriteLine('{"id":1,"method":"getState","params":{}}')
    $liveRuntime.StandardInput.Flush()
    $ready = $liveRuntime.StandardOutput.ReadLineAsync()
    if (-not $ready.Wait(5000) -or ($ready.Result | ConvertFrom-Json).result.protocolVersion -ne 2) { throw 'Existing Rust runtime did not initialize' }
    & (Join-Path $qaTarget 'MiniCode.exe') -manifest $manifestPath -archive $archivePath -target $qaTarget -expected (Join-Path $installationPath 'MiniCode.exe') -port $qaPort
    if ($LASTEXITCODE -ne 0) { throw 'Update SDK install/tamper probe failed' }
    $liveRuntime.StandardInput.WriteLine('{"id":2,"method":"getState","params":{}}')
    $liveRuntime.StandardInput.Flush()
    $response = $liveRuntime.StandardOutput.ReadLineAsync()
    if (-not $response.Wait(5000) -or ($response.Result | ConvertFrom-Json).result.protocolVersion -ne 2) { throw 'Update interrupted the existing Rust runtime' }
} finally {
    $liveRuntime.StandardInput.Close()
    if (-not $liveRuntime.WaitForExit(5000)) { $liveRuntime.Kill(); $liveRuntime.WaitForExit() }
    $liveRuntime.Dispose()
}
& (Join-Path $PSScriptRoot 'smoke-native.ps1') -Installation $qaTarget
Write-Host "Signed update, tamper rejection and updated runtime verified: $qaTarget"
