package main

import (
	"bytes"
	"testing"
	"time"

	"github.com/egoist/mygo/ui"
)

func TestRingRespectsReducedMotion(t *testing.T) {
	previous := appReduceMotion
	t.Cleanup(func() { appReduceMotion = previous })
	for _, tc := range []struct {
		name        string
		app, system bool
	}{
		{"animated", false, false},
		{"app preference", true, false},
		{"system preference", false, true},
	} {
		t.Run(tc.name, func(t *testing.T) {
			appReduceMotion = tc.app
			view := ui.NewTester(func(c *ui.Context) {
				ring(c, 24, ui.Hex("#d97757"))
			}, 32, 32)
			view.SetPreferences(ui.Preferences{ReduceMotion: tc.system})
			view.Frame()
			still := tc.app || tc.system
			if view.FramePending() == still {
				t.Fatalf("animation pending = %v, reduced motion = %v", view.FramePending(), still)
			}
			before := bytes.Clone(view.Image().Pix)
			view.Advance(137 * time.Millisecond)
			view.Frame()
			if bytes.Equal(before, view.Image().Pix) != still {
				t.Fatalf("ring image did not respect reduced motion = %v", still)
			}
		})
	}
}
