param([switch]$SkipTests)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$root = Split-Path $PSScriptRoot -Parent
$stage = Join-Path $PSScriptRoot '.stage'
$cache = Join-Path $PSScriptRoot '.cache'
$originalPath = $env:PATH
$originalSigningKey = $env:MYGO_UPDATER_PRIVATE_KEY
function Run([string]$Program, [string[]]$Arguments) {
    & $Program @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Program failed ($LASTEXITCODE)" }
}
Push-Location $root
try {
    if (-not $IsWindows -or [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture -ne 'X64') {
        throw 'The installer must be built on Windows x64 with PowerShell 7.'
    }
    if (-not $env:MYGO_UPDATER_PRIVATE_KEY) {
        $signingFile = Join-Path ([Environment]::GetFolderPath('ApplicationData')) 'mygo/update-keys/mygo-update.key'
        if (-not (Test-Path -LiteralPath $signingFile)) { throw 'Set MYGO_UPDATER_PRIVATE_KEY to the existing update signing key before building a release.' }
        $env:MYGO_UPDATER_PRIVATE_KEY = (Get-Content -LiteralPath $signingFile -Raw).Trim()
    }
    # Only release staging is cleared; caches and personal data are never copied.
    if ([IO.Path]::GetFullPath($stage) -ne [IO.Path]::Combine([IO.Path]::GetFullPath($PSScriptRoot), '.stage')) { throw 'Invalid staging path' }
    if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
    New-Item -ItemType Directory -Force "$stage/runtime", "$stage/wheels", $cache | Out-Null
    Run 'npm.cmd' @('ci', '--prefix', 'tui', '--no-audit', '--no-fund')
    Run 'npm.cmd' @('run', 'build', '--prefix', 'tui')
    New-Item -ItemType Directory -Force "$stage/runtime/tui" | Out-Null
    Copy-Item -LiteralPath "$root/tui/dist", "$root/tui/package.json", "$root/tui/package-lock.json" -Destination "$stage/runtime/tui" -Recurse
    Run 'npm.cmd' @('ci', '--prefix', "$stage/runtime/tui", '--omit=dev', '--no-audit', '--no-fund')
    $version = (Get-Content "$PSScriptRoot/package.json" -Raw | ConvertFrom-Json).version
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
    $nodeVersion = '22.22.0'
    $nodeZip = "$cache/node-v$nodeVersion-win-x64.zip"
    if (-not (Test-Path -LiteralPath $nodeZip)) {
        Invoke-WebRequest "https://nodejs.org/dist/v$nodeVersion/node-v$nodeVersion-win-x64.zip" -OutFile $nodeZip
    }
    if ((Get-FileHash $nodeZip -Algorithm SHA256).Hash.ToLowerInvariant() -ne 'c97fa376d2becdc8863fcd3ca2dd9a83a9f3468ee7ccf7a6d076ec66a645c77a') { throw 'Node.js checksum mismatch' }
    Expand-Archive -LiteralPath $nodeZip -DestinationPath "$stage/node-download"
    New-Item -ItemType Directory -Force "$stage/runtime/node" | Out-Null
    Copy-Item -LiteralPath "$stage/node-download/node-v$nodeVersion-win-x64/node.exe", "$stage/node-download/node-v$nodeVersion-win-x64/LICENSE" -Destination "$stage/runtime/node"
    # Acceptance commands such as `python -m pytest` must use the same Python
    # as the packaged hosts, including on runners with another Python installed.
    $env:PATH = "$stage/runtime/python;$stage/runtime/python/Scripts;$stage/runtime/git/cmd;$originalPath"
    Run $python @('-I', '-X', 'utf8', "$PSScriptRoot/audit.py", '--source', '--history', $root)
    Run $python @('-I', '-X', 'utf8', "$PSScriptRoot/audit.py", "$stage/runtime")
    if (-not $SkipTests) {
        Push-Location "$root/desktop/native"
        try { Run 'go' @('test', './...') } finally { Pop-Location }
        Run $python @('-m', 'pytest', 'tests', '-o', 'addopts=', '-q', '--basetemp', "$cache/tests")
        $oldPython = $env:MINICODE_PYTHON
        $oldBridgeTest = $env:MINICODE_TUI_BRIDGE_TEST
        try {
            $env:MINICODE_PYTHON = $python
            $env:MINICODE_TUI_BRIDGE_TEST = '1'
            Run 'npm.cmd' @('test', '--prefix', 'tui')
        } finally {
            $env:MINICODE_PYTHON = $oldPython
            $env:MINICODE_TUI_BRIDGE_TEST = $oldBridgeTest
        }
    }
    Push-Location "$root/desktop/native"
    try {
        Run 'go' @('run', './tools/icon', 'assets/app-mark.svg', 'resources/icon.png')
        $licenseTarget = "$root/desktop/native/resources/licenses"
        New-Item -ItemType Directory -Force $licenseTarget | Out-Null
        $modules = @(Get-ChildItem -LiteralPath "$root/desktop/native/vendor" -File -Recurse |
            Where-Object { $_.Name -match '^LICENSE|^COPYING|^NOTICE' })
        foreach ($licenseFile in $modules) {
            $relative = [IO.Path]::GetRelativePath("$root/desktop/native/vendor", $licenseFile.FullName)
            $destination = Join-Path $licenseTarget $relative
            New-Item -ItemType Directory -Force (Split-Path $destination -Parent) | Out-Null
            Copy-Item -LiteralPath $licenseFile.FullName -Destination $destination
        }
        $env:CGO_ENABLED = '0'
        Run 'mygo' @('build', '-platform', 'windows/amd64')
    }
    finally { Pop-Location }
    $installation = "$PSScriptRoot/dist/native/windows-amd64"
    Run 'pwsh' @('-NoProfile', '-File', "$PSScriptRoot/smoke-native.ps1", '-Installation', $installation)
    Run $python @('-I', '-X', 'utf8', "$PSScriptRoot/audit.py", $installation)
    Run $python @('-I', '-X', 'utf8', "$PSScriptRoot/verify-update.py", "$installation/update-windows-amd64.json", "$root/desktop/native/mygo.json")
    Run 'pwsh' @('-NoProfile', '-File', "$PSScriptRoot/test-updater.ps1", '-Installation', $installation)
    Run 'pwsh' @('-NoProfile', '-File', "$PSScriptRoot/test-installer.ps1", '-Installation', $installation)
    $builtInstaller = Get-Item "$installation/MiniCode Setup $version.exe"
    $installerPath = "$PSScriptRoot/dist/MiniCode-Setup-$version-win-x64.exe"
    Copy-Item -LiteralPath $builtInstaller.FullName -Destination $installerPath
    Copy-Item -LiteralPath "$installation/update-windows-amd64.json" -Destination "$PSScriptRoot/dist"
    Get-ChildItem -LiteralPath $installation -File | Where-Object { $_.Name -like '*.tar.gz' -or $_.Name -like '*.delta' } | Copy-Item -Destination "$PSScriptRoot/dist"
    $installer = Get-Item $installerPath
    $hash = (Get-FileHash $installer.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    "$hash  $($installer.Name)" | Set-Content "$PSScriptRoot/dist/SHA256SUMS.txt" -Encoding ascii
    Write-Host "Installer verified: $($installer.FullName)"
} finally { $env:PATH = $originalPath; $env:MYGO_UPDATER_PRIVATE_KEY = $originalSigningKey; Pop-Location }
