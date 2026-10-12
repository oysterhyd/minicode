param([string]$Installation = (Join-Path $PSScriptRoot 'dist/native/windows-amd64'))
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$installationPath = [IO.Path]::GetFullPath($Installation)
$runtime = Join-Path $installationPath 'runtime'
$agent = Join-Path $runtime 'minicode.exe'
$node = Join-Path $runtime 'node/node.exe'
$version = (Get-Content (Join-Path $PSScriptRoot 'package.json') -Raw | ConvertFrom-Json).version
$scratch = Join-Path $PSScriptRoot ('.cache/native-smoke-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force $scratch | Out-Null
$original = @{}
foreach ($key in @('USERPROFILE','HOME','APPDATA','LOCALAPPDATA','PATH','MINICODE_RESOURCES','MINICODE_AGENT')) {
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
    $env:MINICODE_RESOURCES = $installationPath
    $env:PATH = "$runtime/node;$runtime/git/cmd;$env:SystemRoot/System32;$env:SystemRoot/System32/WindowsPowerShell/v1.0"
    Push-Location $scratch
    try {
        $actual = (& $agent --version).Trim().Split(' ')[-1]
        if ($LASTEXITCODE -ne 0 -or $actual -ne $version) { throw "Packaged Rust version mismatch: $actual" }
        Run $agent @('--help')
        Run (Join-Path $installationPath 'minicode.cmd') @('--help')
        Run (Join-Path $runtime 'git/cmd/git.exe') @('--version')
        Run $agent @('eval','--task','pagination_bounds','--baselines','b0,b2','--output','results')
        $results = Get-Content (Join-Path $scratch 'results/results.json') -Raw | ConvertFrom-Json
        if ($results.run_count -ne 2 -or $results.passed -ne 2) { throw 'Installed CLI evaluation failed' }
        $env:MINICODE_AGENT = $agent
        $tuiProbe = "const {BridgeClient}=await import('./dist/bridge.js');const {spawnBridge}=await import('./dist/launch.js');const c=new BridgeClient(spawnBridge({}));try{const state=await c.request('getState');if(state.protocolVersion!==2)throw Error('protocol');const config=await c.request('getConfiguration');const models=await c.request('listModels');if(state.model!==''||config.services.length!==0||config.defaultModel!==''||models.length!==0)throw Error('fresh model configuration must be empty');await c.request('setTuiProvider',{provider:'fake',model:'fake'});}finally{await c.close();}console.log('Packaged TUI bridge and empty model configuration passed');"
        Push-Location (Join-Path $runtime 'tui')
        try {
            Run $node @('dist/index.js', '--help')
            Run $node @('--input-type=module','-e',$tuiProbe)
        } finally { Pop-Location }
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
        Write-Host "Native window capture, bundled Rust bridge, CLI, Git and TUI verified: $report"
    } finally { Pop-Location }
} finally {
    foreach ($key in $original.Keys) { [Environment]::SetEnvironmentVariable($key, $original[$key], 'Process') }
}
