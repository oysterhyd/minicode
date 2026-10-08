param([string]$Installation = (Join-Path $PSScriptRoot 'dist/native/windows-amd64'))
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$installationPath = [IO.Path]::GetFullPath($Installation)
$runtime = Join-Path $installationPath 'runtime'
$python = Join-Path $runtime 'python/python.exe'
$node = Join-Path $runtime 'node/node.exe'
$version = (Get-Content (Join-Path $PSScriptRoot 'package.json') -Raw | ConvertFrom-Json).version
$scratch = Join-Path $PSScriptRoot ('.cache/native-smoke-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force $scratch | Out-Null
$original = @{}
foreach ($key in @('USERPROFILE','HOME','APPDATA','LOCALAPPDATA','PATH','PYTHONHOME','PYTHONPATH','PYTHONUTF8','PYTHONIOENCODING','MINICODE_RESOURCES','MINICODE_PYTHON','MINICODE_BRIDGE')) {
    $original[$key] = [Environment]::GetEnvironmentVariable($key, 'Process')
}
function Run([string]$Program,[string[]]$Arguments) {
    & $Program @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Program failed ($LASTEXITCODE)" }
}
try {
    $env:USERPROFILE = $scratch
    $env:HOME = $scratch
    $env:APPDATA = Join-Path $scratch 'AppData/Roaming'
    $env:LOCALAPPDATA = Join-Path $scratch 'AppData/Local'
    New-Item -ItemType Directory -Force $env:APPDATA,$env:LOCALAPPDATA | Out-Null
    $env:PYTHONHOME = ''
    $env:PYTHONPATH = ''
    $env:PYTHONUTF8 = '1'
    $env:PYTHONIOENCODING = 'utf-8'
    $env:MINICODE_RESOURCES = $installationPath
    $env:PATH = "$runtime/python;$runtime/python/Scripts;$runtime/git/cmd;$env:SystemRoot/System32;$env:SystemRoot/System32/WindowsPowerShell/v1.0"
    Push-Location $scratch
    try {
        $actual = (& $python -I -c 'import minicode; print(minicode.__version__)').Trim()
        if ($LASTEXITCODE -ne 0 -or $actual -ne $version) { throw "Packaged Python version mismatch: $actual" }
        Run $python @('-I','-X','utf8','-m','minicode','--help')
        Run (Join-Path $installationPath 'minicode.cmd') @('--help')
        Run (Join-Path $runtime 'git/cmd/git.exe') @('--version')
        Run $python @('-I','-X','utf8','-m','minicode','eval','--task','pagination_bounds','--baselines','b0,b2','--output','results')
        $results = Get-Content (Join-Path $scratch 'results/results.json') -Raw | ConvertFrom-Json
        if ($results.run_count -ne 2 -or $results.passed -ne 2) { throw 'Installed CLI evaluation failed' }
        $env:MINICODE_PYTHON = $python
        $env:MINICODE_BRIDGE = Join-Path $runtime 'bridge.py'
        $tuiProbe = "const {BridgeClient}=await import('./dist/bridge.js');const {spawnBridge}=await import('./dist/launch.js');const c=new BridgeClient(spawnBridge({}));try{const state=await c.request('getState');if(state.protocolVersion!==2)throw Error('protocol');await c.request('setTuiProvider',{provider:'fake',model:'fake'});}finally{await c.close();}console.log('Packaged TUI bridge passed');"
        Push-Location (Join-Path $runtime 'tui')
        try { Run $node @('--input-type=module','-e',$tuiProbe) } finally { Pop-Location }
        $env:PATH = "$env:SystemRoot/System32"
        $report = Join-Path $scratch 'native-probe.json'
        $exe = Join-Path $installationPath 'MiniCode.exe'
        $process = Start-Process -FilePath $exe -ArgumentList '--probe', ('"{0}"' -f $report) -WorkingDirectory $scratch -WindowStyle Hidden -PassThru
        if (-not $process.WaitForExit(30000)) { $process.Kill(); throw 'Installed native Desktop did not finish its probe' }
        if (-not (Test-Path -LiteralPath $report)) { throw 'Installed native probe did not create a report' }
        $native = Get-Content -LiteralPath $report -Raw | ConvertFrom-Json
        if (-not $native.native -or $native.protocol -ne 2 -or $native.capture_bytes -le 0) { throw 'Installed native window/bridge failed' }
        if (-not $native.updates_enabled) { throw 'Installed native updater is not enabled' }
        if ($native.PSObject.Properties.Name -contains 'error') { throw $native.error }
        Write-Host "Native window capture, bundled Python bridge, CLI, Git and TUI verified: $report"
    } finally { Pop-Location }
} finally {
    foreach ($key in $original.Keys) { [Environment]::SetEnvironmentVariable($key, $original[$key], 'Process') }
}
