param([switch]$SkipTests)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$root = Split-Path $PSScriptRoot -Parent
$stage = Join-Path $PSScriptRoot '.stage'
$cache = Join-Path $PSScriptRoot '.cache'
$originalPath = $env:PATH
$originalSigningKey = $env:MYGO_UPDATER_PRIVATE_KEY
$originalBenchOutput = $env:MINICODE_BENCH_OUTPUT
function Run([string]$Program, [string[]]$Arguments) {
    & $Program @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Program failed ($LASTEXITCODE)" }
}
Push-Location $root
try {
    if (-not $IsWindows -or [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture -ne 'X64') { throw 'Build on Windows x64 with PowerShell 7.' }
    if (-not $env:MYGO_UPDATER_PRIVATE_KEY) {
        $key = Join-Path ([Environment]::GetFolderPath('ApplicationData')) 'mygo/update-keys/mygo-update.key'
        if (-not (Test-Path -LiteralPath $key)) { throw 'Set MYGO_UPDATER_PRIVATE_KEY to the existing update signing key.' }
        $env:MYGO_UPDATER_PRIVATE_KEY = (Get-Content -LiteralPath $key -Raw).Trim()
    }
    if ([IO.Path]::GetFullPath($stage) -ne [IO.Path]::Combine([IO.Path]::GetFullPath($PSScriptRoot), '.stage')) { throw 'Invalid staging path' }
    if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
    New-Item -ItemType Directory -Force "$stage/runtime/tui",$cache | Out-Null
    Run 'cargo' @('build','--release','--locked')
    Copy-Item -LiteralPath "$root/target/release/minicode.exe" -Destination "$stage/runtime/minicode.exe"
    Run 'npm.cmd' @('ci','--prefix','tui','--no-audit','--no-fund')
    Run 'npm.cmd' @('run','build','--prefix','tui')
    Copy-Item -LiteralPath "$root/tui/dist","$root/tui/package.json","$root/tui/package-lock.json" -Destination "$stage/runtime/tui" -Recurse
    Run 'npm.cmd' @('ci','--prefix',"$stage/runtime/tui",'--omit=dev','--no-audit','--no-fund')
    Run 'node' @("$PSScriptRoot/prune-runtime.mjs","$stage/runtime")
    New-Item -ItemType Directory -Force "$root/output" | Out-Null
    Run 'node' @("$PSScriptRoot/test-tui-runtime.mjs","$stage/runtime")
    Run 'node' @("$PSScriptRoot/collect-rust-licenses.mjs","$stage/runtime/licenses/rust")
    New-Item -ItemType Directory -Force "$stage/runtime/evals" | Out-Null
    Copy-Item -LiteralPath "$root/evals/tasks" -Destination "$stage/runtime/evals/tasks" -Recurse
    Copy-Item -LiteralPath "$root/evals/scenarios" -Destination "$stage/runtime/evals/scenarios" -Recurse
    Copy-Item -LiteralPath "$root/evals/hidden_tests" -Destination "$stage/runtime/evals/hidden_tests" -Recurse
    Copy-Item -LiteralPath "$root/LICENSE","$PSScriptRoot/THIRD-PARTY-NOTICES.md" -Destination "$stage/runtime"
    $gitZip = "$cache/MinGit-2.56.0-64-bit.zip"
    if (-not (Test-Path -LiteralPath $gitZip)) { Invoke-WebRequest 'https://github.com/git-for-windows/git/releases/download/v2.56.0.windows.1/MinGit-2.56.0-64-bit.zip' -OutFile $gitZip }
    if ((Get-FileHash $gitZip -Algorithm SHA256).Hash.ToLowerInvariant() -ne '064b440ff870ed5198527e8f3a92cdf5bd2fd0fedf5e718af95e3fdaddeff718') { throw 'MinGit checksum mismatch' }
    Expand-Archive -LiteralPath $gitZip -DestinationPath "$stage/runtime/git"
    $nodeVersion = '22.22.0'
    $nodeZip = "$cache/node-v$nodeVersion-win-x64.zip"
    if (-not (Test-Path -LiteralPath $nodeZip)) { Invoke-WebRequest "https://nodejs.org/dist/v$nodeVersion/node-v$nodeVersion-win-x64.zip" -OutFile $nodeZip }
    if ((Get-FileHash $nodeZip -Algorithm SHA256).Hash.ToLowerInvariant() -ne 'c97fa376d2becdc8863fcd3ca2dd9a83a9f3468ee7ccf7a6d076ec66a645c77a') { throw 'Node.js checksum mismatch' }
    Expand-Archive -LiteralPath $nodeZip -DestinationPath "$stage/node-download"
    New-Item -ItemType Directory -Force "$stage/runtime/node" | Out-Null
    Copy-Item -LiteralPath "$stage/node-download/node-v$nodeVersion-win-x64/node.exe","$stage/node-download/node-v$nodeVersion-win-x64/LICENSE" -Destination "$stage/runtime/node"
    $env:PATH = "$stage/runtime/node;$stage/runtime/git/cmd;$originalPath"
    Run 'node' @("$PSScriptRoot/audit.mjs",'--source','--history',$root)
    Run 'node' @("$PSScriptRoot/audit.mjs","$stage/runtime")
    if (-not $SkipTests) {
        if (-not $env:MINICODE_BENCH_OUTPUT) { $env:MINICODE_BENCH_OUTPUT = "$cache/algorithm-measurements.json" }
        Run 'cargo' @('test','--locked')
        Run 'cargo' @('fmt','--check')
        Run 'cargo' @('clippy','--locked','--all-targets','--','-D','warnings')
        Run 'node' @('evals/generate-tasks.mjs','--verify')
        $oldAgent=$env:MINICODE_AGENT
        $oldBridgeTest=$env:MINICODE_TUI_BRIDGE_TEST
        try {
            $env:MINICODE_AGENT="$stage/runtime/minicode.exe"
            $env:MINICODE_TUI_BRIDGE_TEST='1'
            Run 'node' @('--test','tests/agent/*.test.mjs')
            $evalOutput="$cache/eval-$([guid]::NewGuid().ToString('N'))"
            $scenarioOutput="$cache/scenarios-$([guid]::NewGuid().ToString('N'))"
            Run $env:MINICODE_AGENT @('eval','--baselines','b0,b1,b2','--concurrency','3','--output',$evalOutput)
            Run $env:MINICODE_AGENT @('eval','--tasks-dir','evals/scenarios','--baselines','b0,b1,b2','--concurrency','3','--output',$scenarioOutput)
            Run 'node' @('evals/verify-results.mjs',$evalOutput,$scenarioOutput)
            Run 'npm.cmd' @('test','--prefix','tui')
            Push-Location "$root/desktop/native"
            try { Run 'go' @('test','-count=1','./...'); Run 'go' @('test','-count=1','./vendor/github.com/egoist/mygo/ui') } finally { Pop-Location }
        } finally { $env:MINICODE_AGENT=$oldAgent; $env:MINICODE_TUI_BRIDGE_TEST=$oldBridgeTest }
    }
    Push-Location "$root/desktop/native"
    try {
        Run 'go' @('run','./tools/icon','assets/app-mark.svg','resources/icon.png')
        $licenseTarget="$root/desktop/native/resources/licenses"
        New-Item -ItemType Directory -Force $licenseTarget | Out-Null
        foreach ($file in @(Get-ChildItem -LiteralPath "$root/desktop/native/vendor" -File -Recurse | Where-Object { $_.Name -match '^LICENSE|^COPYING|^NOTICE' })) {
            $relative=[IO.Path]::GetRelativePath("$root/desktop/native/vendor",$file.FullName)
            $destination=Join-Path $licenseTarget $relative
            New-Item -ItemType Directory -Force (Split-Path $destination -Parent) | Out-Null
            Copy-Item -LiteralPath $file.FullName -Destination $destination
        }
        $env:CGO_ENABLED='0'
        Run 'mygo' @('build','-platform','windows/amd64')
    } finally { Pop-Location }
    $installation="$PSScriptRoot/dist/native/windows-amd64"
    Run 'pwsh' @('-NoProfile','-File',"$PSScriptRoot/smoke-native.ps1",'-Installation',$installation)
    Run 'node' @("$PSScriptRoot/audit.mjs",$installation)
    Run 'node' @("$PSScriptRoot/verify-update.mjs","$installation/update-windows-amd64.json","$root/desktop/native/mygo.json")
    Run 'pwsh' @('-NoProfile','-File',"$PSScriptRoot/test-updater.ps1",'-Installation',$installation)
    Run 'pwsh' @('-NoProfile','-File',"$PSScriptRoot/test-installer.ps1",'-Installation',$installation)
    $version=(Get-Content "$PSScriptRoot/package.json" -Raw|ConvertFrom-Json).version
    $built=Get-Item "$installation/MiniCode Setup $version amd64.exe"
    $installer="$PSScriptRoot/dist/MiniCode-Setup-$version-win-x64.exe"
    Copy-Item -LiteralPath $built.FullName -Destination $installer
    Copy-Item -LiteralPath "$installation/update-windows-amd64.json" -Destination "$PSScriptRoot/dist"
    Get-ChildItem -LiteralPath $installation -File | Where-Object { $_.Name -like '*.tar.gz' -or $_.Name -like '*.delta' } | Copy-Item -Destination "$PSScriptRoot/dist"
    $hash=(Get-FileHash $installer -Algorithm SHA256).Hash.ToLowerInvariant()
    "$hash  $([IO.Path]::GetFileName($installer))" | Set-Content "$PSScriptRoot/dist/SHA256SUMS.txt" -Encoding ascii
    Write-Host "Installer verified: $installer"
} finally { $env:PATH=$originalPath; $env:MYGO_UPDATER_PRIVATE_KEY=$originalSigningKey; $env:MINICODE_BENCH_OUTPUT=$originalBenchOutput; Pop-Location }
