$ErrorActionPreference = 'Stop'
$nativeRoot = Split-Path $PSScriptRoot -Parent
Push-Location $nativeRoot
try {
    go mod vendor
    if ($LASTEXITCODE -ne 0) { throw 'go mod vendor failed' }
    $editor = Join-Path $nativeRoot 'vendor/github.com/egoist/mygo/ui/editor.go'
    $source = Get-Content -LiteralPath $editor -Raw
    if (-not $source.Contains('type editor struct {') -or -not $source.Contains('func (ed *editor) wants(k keyEvent) bool {')) {
        throw 'Mygo editor changed; review the native completion extension before updating'
    }
    $source = $source.Replace('type editor struct {', "type editor struct {`n`tappKeys func(Modifiers, Key) bool")
    $source = $source.Replace('func (ed *editor) wants(k keyEvent) bool {', "func (ed *editor) wants(k keyEvent) bool {`n`tif ed.appKeys != nil && ed.appKeys(k.mods,k.key) { return false }")
    Set-Content -LiteralPath $editor -Value $source -Encoding utf8NoBOM
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'minicode_editor.go.txt') -Destination (Join-Path $nativeRoot 'vendor/github.com/egoist/mygo/ui/minicode_editor.go')
    $element = Join-Path $nativeRoot 'vendor/github.com/egoist/mygo/ui/element.go'
    $elementSource = Get-Content -LiteralPath $element -Raw
    $elementSource = $elementSource.Replace('type Element struct {', "type Element struct {`n`tpaintScale float32")
    Set-Content -LiteralPath $element -Value $elementSource -Encoding utf8NoBOM
    $paint = Join-Path $nativeRoot 'vendor/github.com/egoist/mygo/ui/paint.go'
    $paintSource = Get-Content -LiteralPath $paint -Raw
    $start = $paintSource.IndexOf('func (p *Painter) element(e *Element)')
    $end = $paintSource.IndexOf('// clipRect returns', $start)
    if ($start -lt 0 -or $end -lt 0) { throw 'Mygo painter changed; review the native motion extension' }
    $section = $paintSource.Substring($start, $end - $start)
    $section = $section.Replace('box := Rect{e.x, e.y, e.w, e.h}', "opStart,glyphStart:=len(p.s.Ops),len(p.s.Glyphs)`n`tbox := Rect{e.x, e.y, e.w, e.h}")
    $section = [regex]::Replace($section, 'p\.opacity = saved\r?\n\}', "p.minicodeScale(e,opStart,glyphStart)`n`tp.opacity = saved`n}")
    $paintSource = $paintSource.Substring(0, $start) + $section + $paintSource.Substring($end)
    Set-Content -LiteralPath $paint -Value $paintSource -Encoding utf8NoBOM
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'minicode_motion.go.txt') -Destination (Join-Path $nativeRoot 'vendor/github.com/egoist/mygo/ui/minicode_motion.go')
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'minicode_frame.go.txt') -Destination (Join-Path $nativeRoot 'vendor/github.com/egoist/mygo/ui/minicode_frame.go')
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'minicode_surface_windows.go.txt') -Destination (Join-Path $nativeRoot 'vendor/github.com/egoist/mygo/internal/windows/minicode_surface_windows.go')
    $runtime = Join-Path $nativeRoot 'vendor/github.com/egoist/mygo/ui/runtime.go'
    $runtimeSource = Get-Content -LiteralPath $runtime -Raw
    $nextStart = $runtimeSource.IndexOf('func (rt *engine) next()')
    $nextEnd = $runtimeSource.IndexOf('// repaintNow', $nextStart)
    if ($nextStart -lt 0 -or $nextEnd -lt 0) { throw 'Mygo animation scheduler changed; review frame pacing' }
    $nextSection = $runtimeSource.Substring($nextStart, $nextEnd-$nextStart).Replace('rt.host.requestFrame()', 'rt.requestAnimatedFrame()')
    $runtimeSource = $runtimeSource.Substring(0,$nextStart)+$nextSection+$runtimeSource.Substring($nextEnd)
    $runtimeSource = [regex]::Replace($runtimeSource, '(?s)(func \(rt \*engine\) changed\(\) \{.*?rt\.host\.requestFrame\(\))', '$1' + "`n`tif host, ok := rt.host.(interface{ immediateFrame() }); ok { host.immediateFrame() }")
    Set-Content -LiteralPath $runtime -Value $runtimeSource -Encoding utf8NoBOM
    $input = Join-Path $nativeRoot 'vendor/github.com/egoist/mygo/ui/input.go'
    $inputSource = Get-Content -LiteralPath $input -Raw
    $inputSource = [regex]::Replace($inputSource, '(?m)^\treturn taken\r?\n\}', "`tif ev.Kind != platform.SurfaceFrame && ev.Kind != platform.PointerMove && ev.Kind != platform.PointerLeave {`n`t`tif host, ok := rt.host.(interface{ immediateFrame() }); ok { host.immediateFrame() }`n`t}`n`treturn taken`n}")
    Set-Content -LiteralPath $input -Value $inputSource -Encoding utf8NoBOM
    $windowHost = Join-Path $nativeRoot 'vendor/github.com/egoist/mygo/ui/window.go'
    $windowSource = (Get-Content -LiteralPath $windowHost -Raw).Replace('type windowHost struct {', "type windowHost struct {`n`tminicodeFrameTimer *time.Timer`n`tminicodeFrameDue time.Time")
    Set-Content -LiteralPath $windowHost -Value $windowSource -Encoding utf8NoBOM
    $d3d = Join-Path $nativeRoot 'vendor/github.com/egoist/mygo/internal/gpu/d3d11/d3d11.go'
    $d3dSource = (Get-Content -LiteralPath $d3d -Raw).Replace('sync := uintptr(1)', 'sync := uintptr(0) // DWM composes; UI thread does not wait for vertical blank.')
    Set-Content -LiteralPath $d3d -Value $d3dSource -Encoding utf8NoBOM
    gofmt -w $editor $element $paint (Join-Path $nativeRoot 'vendor/github.com/egoist/mygo/ui/minicode_editor.go') (Join-Path $nativeRoot 'vendor/github.com/egoist/mygo/ui/minicode_motion.go')
    if ($LASTEXITCODE -ne 0) { throw 'formatting the toolkit extension failed' }
    gofmt -w $runtime $input $windowHost $d3d (Join-Path $nativeRoot 'vendor/github.com/egoist/mygo/ui/minicode_frame.go') (Join-Path $nativeRoot 'vendor/github.com/egoist/mygo/internal/windows/minicode_surface_windows.go')
    if ($LASTEXITCODE -ne 0) { throw 'formatting the frame extension failed' }
} finally { Pop-Location }
