package main

import (
	"math"
	"time"

	"github.com/egoist/mygo/ui"
)

// Both permission surfaces use the same left-aligned radio row.
func permissionChoice(c *ui.Context, p palette, value string, index int, compact bool) ui.Element {
	option := permissionOptions[index]
	selected := value
	row := ui.RadioBase(c, &selected, option.Value).Label(tr(option.Label)).Description(tr(option.Detail)).
		FillWidth().Justify(ui.Start).Padding(12, 14).Gap(12).Radius(14).Border(1, p.BorderSubtle).Transition(fastMotion)
	if compact {
		row.Border(0, ui.Transparent).Padding(10).Radius(12)
	}
	active := value == option.Value
	if active {
		row.Background(p.Elevated).BorderColor(p.BorderStrong)
		if !compact {
			row.Shadow(0, 1, 3, 0, ui.RGBA(0, 0, 0, 0.06))
		} else {
			row.Background(p.Active)
		}
	} else if row.Hovered() {
		row.Background(p.Hover)
	}
	row.Children(func() {
		name := []string{"Shield", "ShieldCheck", "ShieldOff"}[index]
		color := p.Text3
		if active {
			color = p.Text
		}
		ui.Box(c).Size(30, 30).Shrink(0).Radius(9).Background(p.Subtle).Border(1, p.BorderSubtle).Center().Children(func() {
			icon(c, name, 15, color)
		})
		ui.Column(c).Grow(1).Basis(0).MinWidth(0).Gap(4).Children(func() {
			ui.Text(c, tr(option.Label)).TextAlign(ui.Start).FontSize(p.font(12.04)).FontWeight(600)
			muted(c, p, option.Detail).TextAlign(ui.Start).FontSize(p.font(10.92)).LineHeight(1.5)
		})
		ring := p.BorderStrong
		if active {
			ring = p.Ink
		}
		ui.Box(c).Size(18, 18).Shrink(0).Radius(99).Border(1.5, ring).Center().Transition(fastMotion).Children(func() {
			dot := ui.Box(c).Size(8, 8).Radius(99).Background(p.Ink)
			pose := ui.Pose{Scale: 0.2, Fade: 1}
			if active {
				pose = ui.Pose{Scale: 1}
			}
			dot.AnimatePose(pose, pressMotion)
		})
	})
	return row
}

type effortSliderState struct {
	Value    float64
	External string
	Dragging bool
}

func effortSlider(c *ui.Context, p palette, value string, supported bool, changed func(string)) {
	root := ui.Column(c).FillWidth().Gap(4)
	state := ui.Local(root, "effort", func() effortSliderState { return effortSliderState{} })
	if !state.Dragging && state.External != value {
		state.Value = 0
		for i, option := range effortOptions {
			if option.Value == value {
				state.Value = float64(i)
			}
		}
		state.External = value
	}
	root.Children(func() {
		ui.Row(c).Children(func() {
			muted(c, p, tr("思考强度")).Grow(1)
			label := effortOptions[int(math.Round(state.Value))].Label
			if !supported {
				label = tr("不可用")
			}
			badge(c, p, label, p.Text2, p.Active)
		})
		ui.Column(c).FillWidth().Disabled(!supported).Children(func() {
			slider := ui.SliderBase(c, &state.Value, 0, float64(len(effortOptions)-1)).Step(1).
				Label(tr("思考强度滑条")).Height(32).FillWidth().PaddingX(12).Radius(8)
			position := slider.Animate("effort-position", float32(state.Value)/float32(len(effortOptions)-1), 100*time.Millisecond)
			slider.Draw(func(canvas *ui.Painter, r ui.Rect) {
				track := ui.Rect{X: r.X + 12, Y: r.Y + r.H/2 - 3, W: r.W - 24, H: 6}
				fill := p.Ink
				if !supported {
					fill = p.Text4
				}
				canvas.Fill(track, p.Active, 3)
				_ = fill
				x := track.X + track.W*position
				canvas.Fill(ui.Rect{X: track.X, Y: track.Y, W: x - track.X, H: track.H}, fill, 3)
				for i := range effortOptions {
					tx := track.X + track.W*float32(i)/float32(len(effortOptions)-1)
					tick := p.BorderStrong
					if tx <= x {
						tick = p.InkText.Alpha(0.6)
					}
					canvas.Fill(ui.Rect{X: tx - 1.5, Y: track.Y + 1.5, W: 3, H: 3}, tick, 2)
				}
				// A capsule thumb, white and raised, as a macOS slider's.
				thumb := ui.Rect{X: x - 11, Y: r.Y + r.H/2 - 8, W: 22, H: 16}
				canvas.Shadow(thumb, 8, 0, 1, 4, 0, ui.RGBA(0, 0, 0, 0.2))
				canvas.Fill(thumb, p.Thumb, 8)
				canvas.Stroke(thumb, p.Border, 8, 1)
			})
			commit := slider.Changed() && !slider.Pressed()
			if slider.Pressed() {
				state.Dragging = true
			} else if state.Dragging {
				state.Dragging = false
				commit = true
			}
			if supported && commit {
				changed(effortOptions[int(math.Round(state.Value))].Value)
			}
			ui.Row(c).FillWidth().Children(func() {
				for i, option := range effortOptions {
					b := ui.ButtonBase(c).Label(tr("思考强度：") + tr(option.Label)).Grow(1).Basis(0).Height(26).Radius(8).Center().FontSize(p.font(10.5)).TextColor(p.Text3)
					if i == int(math.Round(state.Value)) && supported {
						b.TextColor(p.Text).FontWeight(600)
					} else if b.Hovered() {
						b.Background(p.Hover)
					}
					b.Children(func() { ui.Text(c, tr(option.Label)).NoWrap() })
					if b.Clicked() && supported {
						state.Value = float64(i)
						changed(option.Value)
					}
				}
			})
		})
		if !supported {
			muted(c, p, tr("选择支持思考强度的模型后可调整")).FontSize(p.font(10.36))
		}
	})
}
