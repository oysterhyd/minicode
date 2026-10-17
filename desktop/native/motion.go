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

func toggle(c *ui.Context, p palette, on *bool, label string) ui.Element {
	// An iOS switch: a capsule that fills with ink when on, and a white
	// knob that springs across and stretches while it is pressed.
	e := ui.SwitchBase(c, on).Label(tr(label)).Size(36, 21).Radius(99).Background(p.Active.Mix(p.Text, 0.1)).Transition(fastMotion)
	if *on {
		e.Background(p.Ink)
	}
	pressed := e.Pressed()
	e.Children(func() {
		width := float32(17)
		if pressed {
			width = 21
		}
		knob := ui.Hex("#ffffff")
		if *on && p.Dark {
			knob = p.InkText
		}
		thumb := ui.Box(c).Height(17).Radius(99).Background(knob).Shadow(0, 1, 3, 0, ui.RGBA(0, 0, 0, 0.25)).Absolute().Top(2)
		thumb.Width(thumb.AnimateWith("width", width, 160*time.Millisecond, legacyEase))
		target := float32(2)
		if *on {
			target = 36 - 2 - width
		}
		thumb.Left(thumb.Animate("position", target, 220*time.Millisecond))
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

// pressMotion is how a control sinks under the pointer and comes back.
var pressMotion = ui.Animation{Spring: ui.SpringOptions{Duration: 260 * time.Millisecond, Bounce: 0.25}}

// reduceMotion reports whether the desktop asks for less motion.
func reduceMotion(c *ui.Context) bool { return c.Preferences().ReduceMotion || appReduceMotion }

// appReduceMotion is the app's own "reduce motion" setting, set as the view
// builds.
var appReduceMotion bool

// spark draws Claude's ten-ray asterisk. A live one turns slowly, as the
// mark of work going on, in frames that only paint.
func spark(c *ui.Context, size float32, color ui.Color, live bool) ui.Element {
	still := !live || reduceMotion(c)
	return ui.Box(c).Size(size, size).Shrink(0).Draw(func(painter *ui.Painter, r ui.Rect) {
		turn := 0.0
		if !still {
			painter.AnimationFrame()
			turn = float64(painter.Now().UnixMilli()%4800) / 4800 * 2 * math.Pi
		}
		cx, cy := float64(r.X+r.W/2), float64(r.Y+r.H/2)
		radius := float64(min(r.W, r.H)) / 2
		var path ui.Path
		for i := range 10 {
			a := turn + float64(i)*math.Pi/5 - math.Pi/2
			length := radius
			if i%2 == 1 {
				length *= 0.86
			}
			if live && !still {
				// Each ray breathes a little out of step with its neighbours.
				phase := float64(painter.Now().UnixMilli()%1400)/1400*2*math.Pi + float64(i)*0.9
				length *= 0.9 + 0.1*math.Sin(phase)
			}
			width := radius * 0.17
			ux, uy := math.Cos(a), math.Sin(a)
			nx, ny := -uy, ux
			base := radius * 0.08
			mid := length * 0.42
			path.MoveTo(float32(cx+ux*base), float32(cy+uy*base))
			path.LineTo(float32(cx+ux*mid+nx*width/2), float32(cy+uy*mid+ny*width/2))
			path.LineTo(float32(cx+ux*length), float32(cy+uy*length))
			path.LineTo(float32(cx+ux*mid-nx*width/2), float32(cy+uy*mid-ny*width/2))
			path.Close()
		}
		painter.FillPath(&path, color)
	})
}

// pulse draws a rounded box whose opacity breathes between lo and hi over
// period, as a live status dot or a caret, in frames that only paint.
func pulse(c *ui.Context, w, h, radius float32, color ui.Color, period time.Duration, lo, hi float32) ui.Element {
	still := reduceMotion(c)
	return ui.Box(c).Size(w, h).Shrink(0).Draw(func(painter *ui.Painter, r ui.Rect) {
		alpha := hi
		if !still {
			painter.AnimationFrame()
			t := float64(painter.Now().UnixMilli()%period.Milliseconds()) / float64(period.Milliseconds())
			alpha = lo + (hi-lo)*float32(0.5-0.5*math.Cos(2*math.Pi*t))
		}
		painter.Fill(r, color.Alpha(alpha), radius)
	})
}

// ring draws a thin arc turning around a faint track, the spinner of a
// running step, in frames that only paint.
func ring(c *ui.Context, size float32, color ui.Color) ui.Element {
	return ui.Box(c).Size(size, size).Shrink(0).Draw(func(painter *ui.Painter, r ui.Rect) {
		painter.AnimationFrame()
		cx, cy := r.X+r.W/2, r.Y+r.H/2
		radius := min(r.W, r.H)/2 - 1.2
		var track ui.Path
		track.Circle(cx, cy, radius)
		painter.StrokePath(&track, 1.6, color.Alpha(0.22))
		start := float64(painter.Now().UnixMilli()%900) / 900 * 2 * math.Pi
		var arc ui.Path
		for i := 0; i <= 24; i++ {
			a := start + math.Pi*0.6*float64(i)/24
			x, y := cx+radius*float32(math.Cos(a)), cy+radius*float32(math.Sin(a))
			if i == 0 {
				arc.MoveTo(x, y)
			} else {
				arc.LineTo(x, y)
			}
		}
		painter.StrokePath(&arc, 1.6, color)
	})
}
