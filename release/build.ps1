param([switch]$SkipTests)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$root = Split-Path $PSScriptRoot -Parent
$stage = Join-Path $PSScriptRoot '.stage'
$cache = Join-Path $PSScriptRoot '.cache'
function Run([string]$Program, [string[]]$Arguments) {
    & $Program @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Program failed ($LASTEXITCODE)" }
}
Push-Location $root
try {
    if (-not $IsWindows -or [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture -ne 'X64') {
        throw 'The v1.0.0 installer must be built on Windows x64 with PowerShell 7.'
    }
    # Only release staging is cleared; caches and personal data are never copied.
    if ([IO.Path]::GetFullPath($stage) -ne [IO.Path]::Combine([IO.Path]::GetFullPath($PSScriptRoot), '.stage')) { throw 'Invalid staging path' }
    if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
    New-Item -ItemType Directory -Force "$stage/app", "$stage/runtime", "$stage/wheels", $cache | Out-Null
    Run 'npm.cmd' @('ci', '--prefix', 'desktop', '--no-audit', '--no-fund')
    Run 'npm.cmd' @('run', 'build', '--prefix', 'desktop')
    Copy-Item -LiteralPath "$root/desktop/electron", "$root/desktop/dist" -Destination "$stage/app" -Recurse
    $version = (Get-Content "$PSScriptRoot/package.json" -Raw | ConvertFrom-Json).version
    @{ name = 'minicode'; version = $version; main = 'electron/main.cjs'; description = 'MiniCode Desktop and CLI';
       author = 'MiniCode contributors'; license = 'MIT' } | ConvertTo-Json | Set-Content "$stage/app/package.json" -Encoding utf8NoBOM
    Run 'uv' @('python', 'install', '3.12.12', '--install-dir', "$cache/python", '--no-bin')
    Copy-Item -LiteralPath "$cache/python/cpython-3.12.12-windows-x86_64-none" -Destination "$stage/runtime/python" -Recurse
    $python = "$stage/runtime/python/python.exe"
    Run 'uv' @('pip', 'install', '--python', $python, '--break-system-packages', '--require-hashes', '-r', "$PSScriptRoot/requirements.lock")
    Run 'uv' @('build', '--wheel', '--out-dir', "$stage/wheels")
    $wheel = @(Get-ChildItem "$stage/wheels/minicode-*.whl")
    if ($wheel.Count -ne 1 -or $wheel[0].Name -notlike "minicode-$version-*") { throw 'Wheel/release versions differ' }
    Run 'uv' @('pip', 'install', '--python', $python, '--break-system-packages', '--no-deps', $wheel[0].FullName)
    Copy-Item -LiteralPath "$root/desktop/bridge.py", "$root/LICENSE", "$PSScriptRoot/requirements.lock", "$PSScriptRoot/THIRD-PARTY-NOTICES.md" -Destination "$stage/runtime"
    $gitZip = "$cache/MinGit-2.56.0-64-bit.zip"
    if (-not (Test-Path $gitZip)) {
        Invoke-WebRequest 'https://github.com/git-for-windows/git/releases/download/v2.56.0.windows.1/MinGit-2.56.0-64-bit.zip' -OutFile $gitZip
    }
    if ((Get-FileHash $gitZip -Algorithm SHA256).Hash.ToLowerInvariant() -ne '064b440ff870ed5198527e8f3a92cdf5bd2fd0fedf5e718af95e3fdaddeff718') { throw 'MinGit checksum mismatch' }
    Expand-Archive -LiteralPath $gitZip -DestinationPath "$stage/runtime/git"
    Run $python @('-I', '-X', 'utf8', "$PSScriptRoot/audit.py", '--source', '--history', $root)
    Run $python @('-I', '-X', 'utf8', "$PSScriptRoot/audit.py", "$stage/runtime")
    if (-not $SkipTests) {
        Run 'npm.cmd' @('test', '--prefix', 'desktop')
        Run $python @('-m', 'pytest', 'tests', '-q', '--basetemp', "$cache/tests")
    }
    Push-Location $PSScriptRoot
    try { Run 'node' @('node_modules/electron-builder/cli.js', '--config', 'electron-builder.yml', '--win', '--x64', '--publish', 'never') }
    finally { Pop-Location }
    Run 'node' @("$PSScriptRoot/smoke.cjs", "$PSScriptRoot/dist/win-unpacked")
    Run $python @('-I', '-X', 'utf8', "$PSScriptRoot/audit.py", "$PSScriptRoot/dist/win-unpacked")
    $installer = Get-Item "$PSScriptRoot/dist/MiniCode-Setup-$version-win-x64.exe"
    $hash = (Get-FileHash $installer.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    "$hash  $($installer.Name)" | Set-Content "$PSScriptRoot/dist/SHA256SUMS.txt" -Encoding ascii
    Write-Host "Installer verified: $($installer.FullName)"
} finally { Pop-Location }
