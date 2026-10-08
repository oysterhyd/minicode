package ui

import "github.com/egoist/mygo/internal/scene"

// TransformScale scales painting about an element's center without reflow.
func (e *Element) TransformScale(scale float32) *Element {
	e.paintScale = max(0.01, scale)
	return e
}

// HorizontalScroll enables horizontal scrolling on a virtual list.
func (e *Element) HorizontalScroll() *Element {
	e.flags |= flagScrollX
	return e
}

func (p *Painter) minicodeScale(e *Element, opStart, glyphStart int) {
	scale := e.paintScale
	if scale == 0 || scale == 1 {
		return
	}
	cx, cy := (e.x+e.w/2)*p.scale, (e.y+e.h/2)*p.scale
	rect := func(r scene.Rect) scene.Rect {
		return scene.Rect{X: cx + (r.X-cx)*scale, Y: cy + (r.Y-cy)*scale, W: r.W * scale, H: r.H * scale}
	}
	for i := opStart; i < len(p.s.Ops); i++ {
		op := &p.s.Ops[i]
		op.Rect = rect(op.Rect)
		op.Cast = rect(op.Cast)
		for j := range op.Radii {
			op.Radii[j] *= scale
			op.CastRadii[j] *= scale
			op.Border[j] *= scale
		}
		op.Blur *= scale
		if op.Paint == scene.PaintLinear || op.Paint == scene.PaintOklab {
			op.Gradient[0] = cx + (op.Gradient[0]-cx)*scale
			op.Gradient[1] = cy + (op.Gradient[1]-cy)*scale
			op.Gradient[2] = cx + (op.Gradient[2]-cx)*scale
			op.Gradient[3] = cy + (op.Gradient[3]-cy)*scale
		}
	}
	for i := glyphStart; i < len(p.s.Glyphs); i++ {
		g := &p.s.Glyphs[i]
		g.X = cx + (g.X-cx)*scale
		g.Y = cy + (g.Y-cy)*scale
		g.W *= scale
		g.H *= scale
	}
}
