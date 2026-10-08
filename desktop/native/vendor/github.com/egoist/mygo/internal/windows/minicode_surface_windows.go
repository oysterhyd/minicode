//go:build windows && (amd64 || arm64)

package windows

var minicodeUpdateWindow = user32.NewProc("UpdateWindow")

// UpdateWindow handles the invalid region now instead of waiting for the
// low-priority WM_PAINT queue. It runs only after state/input events, on the
// UI thread; GPU presentation and animation pacing remain unchanged.
func (s *surface) RequestImmediateFrame() {
	procInvalidateRectW.Call(s.hwnd, 0, 0)
	minicodeUpdateWindow.Call(s.hwnd)
}
