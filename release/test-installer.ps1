param([string]$Installation = (Join-Path $PSScriptRoot 'dist/native/windows-amd64'))
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$version = (Get-Content (Join-Path $PSScriptRoot 'package.json') -Raw | ConvertFrom-Json).version
$source = [IO.Path]::GetFullPath($Installation)
$installer = Join-Path $source "MiniCode Setup $version.exe"
$qaCache = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '.cache'))
$scratch = [IO.Path]::GetFullPath((Join-Path $qaCache ('installer-qa-' + [guid]::NewGuid().ToString('N'))))
$target = Join-Path $scratch '本地 安装'
if (-not $target.StartsWith($qaCache+[IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw 'Invalid installer QA path' }
New-Item -ItemType Directory -Force $scratch | Out-Null
$registry = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\com.minicode.desktop'
$registryBackup = Join-Path $scratch 'existing-uninstall.reg'
$hadRegistry = Test-Path -LiteralPath $registry
$shortcuts = @([Environment]::GetFolderPath('Programs'), [Environment]::GetFolderPath('Desktop')) | ForEach-Object { Join-Path $_ 'MiniCode.lnk' }
$backups = @{}
if ($hadRegistry) {
    & reg.exe export 'HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\com.minicode.desktop' $registryBackup /y | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'Cannot back up existing install registration' }
}
foreach ($shortcut in $shortcuts) {
    if (Test-Path -LiteralPath $shortcut) {
        $backup = Join-Path $scratch ([guid]::NewGuid().ToString('N') + '.lnk')
        Copy-Item -LiteralPath $shortcut -Destination $backup
        $backups[$shortcut] = $backup
    }
}
try {
    # NSIS requires /D to be the final, unquoted remainder, including spaces.
    $setup = Start-Process -FilePath $installer -ArgumentList "/S /D=$target" -WindowStyle Hidden -PassThru
    if (-not $setup.WaitForExit(60000)) { $setup.Kill(); throw 'Silent install timed out' }
    if ($setup.ExitCode -ne 0) { throw "Silent install failed ($($setup.ExitCode))" }
    foreach ($file in @('MiniCode.exe','Uninstall.exe','minicode.cmd','runtime/python/python.exe','runtime/node/node.exe')) {
        if (-not (Test-Path -LiteralPath (Join-Path $target $file))) { throw "Installer missing $file" }
    }
    if ((Get-FileHash (Join-Path $target 'MiniCode.exe')).Hash -ne (Get-FileHash (Join-Path $source 'MiniCode.exe')).Hash) { throw 'Installed executable differs' }
    if ((Get-ItemProperty -LiteralPath $registry).InstallLocation -ne $target) { throw 'Incorrect uninstall registration' }
    $shell = New-Object -ComObject WScript.Shell
    if ($shell.CreateShortcut($shortcuts[0]).TargetPath -ne (Join-Path $target 'MiniCode.exe')) { throw 'Incorrect Start Menu shortcut' }
    & (Join-Path $PSScriptRoot 'smoke-native.ps1') -Installation $target
    [pscustomobject]@{version=$version; unicode_space_path=$true; executable_matches=$true; start_menu=$true; uninstall_registration=$true; runtime_smoke=$true} | ConvertTo-Json | Set-Content (Join-Path $scratch 'installer-verification.json') -Encoding utf8
} finally {
    $uninstaller = Join-Path $target 'Uninstall.exe'
    if (Test-Path -LiteralPath $uninstaller) {
        # The resolved install target is constrained above to this QA cache.
        $process = Start-Process -FilePath $uninstaller -ArgumentList '/S' -WindowStyle Hidden -PassThru
        if (-not $process.WaitForExit(30000)) { $process.Kill(); throw 'Uninstaller launcher timed out' }
        $deadline = [DateTime]::UtcNow.AddSeconds(30)
        while ((Test-Path -LiteralPath $target) -and [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 100 }
    }
    # Restore any existing user shortcut/registration after the isolated test.
    foreach ($shortcut in $backups.Keys) { Copy-Item -LiteralPath $backups[$shortcut] -Destination $shortcut -Force }
    if ($hadRegistry) {
        & reg.exe import $registryBackup | Out-Null
        if ($LASTEXITCODE -ne 0) { throw 'Cannot restore existing install registration' }
    }
}
if (Test-Path -LiteralPath $target) { throw 'Uninstaller left the QA install directory' }
if (-not $hadRegistry -and (Test-Path -LiteralPath $registry)) { throw 'Uninstaller left the QA registration' }
Write-Host "NSIS install, Unicode path, runtime, shortcut and uninstall verified: $scratch"
