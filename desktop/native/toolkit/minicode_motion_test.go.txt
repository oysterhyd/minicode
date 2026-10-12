package ui

import (
	"testing"
	"time"
)

func TestMiniCodePaintLoopsDoNotBuildOrLayout(t *testing.T) {
	builds := 0
	var pulse, spin *Element
	svg := MustParseSVG([]byte(`<svg viewBox="0 0 14 14"><path d="M2 2L12 7L2 12Z"/></svg>`))
	tt := NewTester(func(c *Context) {
		builds++
		Column(c).Children(func() {
			pulse = Box(c).Label("pulse").Size(14, 14).Background(Hex("#ff0000")).OpacityLoop("pulse", time.Second, Linear, 0, 1)
			spin = Icon(c, svg).FontSize(14).RotateLoop("spin", time.Second, Linear)
		})
	}, 100, 100)
	now := time.Now()
	tt.rt.clock = func() time.Time { return now }
	pulse.st.anims["pulse"].start = now
	spin.st.anims["spin"].start = now
	before := builds
	now = now.Add(500 * time.Millisecond)
	tt.rt.surfaceFrame()
	if builds != before || pulse.opacity != 0.5 || spin.rotate != 180 {
		t.Fatalf("paint loop rebuilt view or changed easing: builds %d -> %d, alpha %g, rotation %g", before, builds, pulse.opacity, spin.rotate)
	}
	if tt.rt.animating || !tt.rt.repainting {
		t.Fatal("paint loop requested a layout animation")
	}
	tt.SetPreferences(Preferences{ReduceMotion: true})
	if tt.rt.animating || tt.rt.repainting || pulse.opacity != 0 || spin.rotate != 0 {
		t.Fatal("reduced motion did not stop visual loops")
	}
}
