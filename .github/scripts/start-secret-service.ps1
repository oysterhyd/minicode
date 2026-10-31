$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
# Real ephemeral OS keyring for Linux CI; production has no plaintext/test fallback.
sudo apt-get update -qq
sudo apt-get install -y dbus gnome-keyring libsecret-tools
$bus = & /usr/bin/dbus-daemon --session --fork --print-address=1
$env:DBUS_SESSION_BUS_ADDRESS = $bus.Trim()
"DBUS_SESSION_BUS_ADDRESS=$env:DBUS_SESSION_BUS_ADDRESS" | Out-File -FilePath $env:GITHUB_ENV -Append -Encoding utf8
([guid]::NewGuid().ToString()) | & /usr/bin/gnome-keyring-daemon --unlock --components=secrets --daemonize | Out-Null
'ci-probe' | & /usr/bin/secret-tool store --label=MiniCode-CI application org.minicode.ci
$probe = & /usr/bin/secret-tool lookup application org.minicode.ci
if ($probe -ne 'ci-probe') { throw 'Secret Service verification failed' }
& /usr/bin/secret-tool clear application org.minicode.ci
