package main

import (
	"math"
	"time"

	"github.com/egoist/mygo/plugins/glass"
	"github.com/egoist/mygo/ui"
)

type modalBackdrop struct{ tint ui.Color }

func (b modalBackdrop) PaintMaterial(p *ui.Painter, r ui.Rect, radii [4]float32) {
	glass.Blur{Radius: 3}.PaintMaterial(p, r, radii)
	p.Fill(r, b.tint, 0)
}

// The original CSS easing is cubic-bezier(0.2, 0.8, 0.2, 1).
func legacyEase(value float32) float32 {
	x := float64(value)
	lo, hi := 0.0, 1.0
	for i := 0; i < 12; i++ {
		t := (lo + hi) / 2
		v := 3*(1-t)*(1-t)*t*0.2 + 3*(1-t)*t*t*0.2 + t*t*t
		if v < x {
			lo = t
		} else {
			hi = t
		}
	}
	t := (lo + hi) / 2
	return float32(3*(1-t)*(1-t)*t*0.8 + 3*(1-t)*t*t + t*t*t)
}

func toggle(c *ui.Context, p palette, on *bool, label string) *ui.Element {
	e := ui.SwitchBase(c, on).Label(label).Size(32.2, 18.9).Radius(99).Background(p.BorderStrong).Transition(fastMotion)
	if *on {
		e.Background(p.Accent)
	}
	e.Children(func() {
		thumb := ui.Box(c).Size(14.9, 14.9).Radius(99).Background(ui.Hex("#ffffff")).Shadow(0, 1, 3, 0, ui.RGBA(0, 0, 0, 0.3)).Absolute().Top(2)
		target := float32(2)
		if *on {
			target = 15.3
		}
		thumb.Left(thumb.AnimateWith("position", target, 180*time.Millisecond, legacyEase))
	})
	return e
}

func contextRing(c *ui.Context, p palette, percent int) {
	color := p.Text3
	if percent >= 65 {
		color = p.Warning
	}
	if percent >= 85 {
		color = p.Danger
	}
	ui.Box(c).Size(16, 16).Draw(func(painter *ui.Painter, r ui.Rect) {
		cx, cy := r.X+r.W/2, r.Y+r.H/2
		var track ui.Path
		track.Circle(cx, cy, 6.2)
		painter.StrokePath(&track, 2.2, p.BorderStrong)
		value := float32(max(0, min(100, percent))) / 100
		if value == 0 {
			return
		}
		var arc ui.Path
		steps := max(2, int(64*value))
		for i := 0; i <= steps; i++ {
			angle := -math.Pi/2 + 2*math.Pi*float64(value)*float64(i)/float64(steps)
			x, y := cx+6.2*float32(math.Cos(angle)), cy+6.2*float32(math.Sin(angle))
			if i == 0 {
				arc.MoveTo(x, y)
			} else {
				arc.LineTo(x, y)
			}
		}
		painter.StrokePath(&arc, 2.2, color)
	})
}
