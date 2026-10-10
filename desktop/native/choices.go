package main

import (
	"math"
	"time"

	"github.com/egoist/mygo/ui"
)

// Both permission surfaces use the same left-aligned radio row.
func permissionChoice(c *ui.Context, p palette, value string, index int, compact bool) *ui.Element {
	option := permissionOptions[index]
	selected := value
	row := ui.RadioBase(c, &selected, option.Value).Label(option.Label).Description(option.Detail).
		FillWidth().Justify(ui.Start).Padding(12).Gap(12).Radius(10).Border(1, p.Border).Transition(fastMotion)
	if compact {
		row.Border(0, ui.Transparent).Padding(10).Radius(8)
	}
	active := value == option.Value
	if active {
		row.Background(p.AccentSofter).BorderColor(p.Accent)
	} else if row.Hovered() {
		row.Background(p.Hover)
	}
	row.Children(func() {
		name := []string{"Shield", "ShieldCheck", "ShieldOff"}[index]
		color := p.Text3
		if active {
			color = p.Accent
		}
		icon(c, name, 16, color)
		ui.Column(c).Grow(1).Basis(0).MinWidth(0).Gap(4).Children(func() {
			ui.Text(c, option.Label).TextAlign(ui.Start).FontSize(p.font(12.04)).FontWeight(600)
			muted(c, p, option.Detail).TextAlign(ui.Start).FontSize(p.font(10.92)).LineHeight(1.5)
		})
		ui.Box(c).Size(16, 16).Shrink(0).Radius(99).Border(1, color).Center().Children(func() {
			if active {
				ui.Box(c).Size(8, 8).Radius(99).Background(p.Accent)
			}
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
			muted(c, p, "思考强度").Grow(1)
			label := effortOptions[int(math.Round(state.Value))].Label
			if !supported {
				label = "不可用"
			}
			badge(c, p, label, p.AccentText, p.AccentSoft)
		})
		ui.Column(c).FillWidth().Disabled(!supported).Children(func() {
			slider := ui.SliderBase(c, &state.Value, 0, float64(len(effortOptions)-1)).Step(1).
				Label("思考强度滑条").Height(32).FillWidth().PaddingX(12).Radius(8)
			position := slider.Animate("effort-position", float32(state.Value)/float32(len(effortOptions)-1), 100*time.Millisecond)
			slider.Draw(func(canvas *ui.Painter, r ui.Rect) {
				track := ui.Rect{X: r.X + 12, Y: r.Y + r.H/2 - 3, W: r.W - 24, H: 6}
				fill := p.Accent
				if !supported {
					fill = p.Text4
				}
				canvas.Fill(track, p.Active, 3)
				x := track.X + track.W*position
				canvas.Fill(ui.Rect{X: track.X, Y: track.Y, W: x - track.X, H: track.H}, fill, 3)
				for i := range effortOptions {
					tx := track.X + track.W*float32(i)/float32(len(effortOptions)-1)
					canvas.Fill(ui.Rect{X: tx - 1.5, Y: track.Y + 1.5, W: 3, H: 3}, p.Main, 2)
				}
				thumb := ui.Rect{X: x - 8, Y: r.Y + r.H/2 - 8, W: 16, H: 16}
				canvas.Shadow(thumb, 8, 0, 1, 4, 0, ui.RGBA(0, 0, 0, 0.18))
				canvas.Fill(thumb, p.Main, 8)
				canvas.Fill(ui.Rect{X: x - 4, Y: r.Y + r.H/2 - 4, W: 8, H: 8}, fill, 4)
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
					b := ui.ButtonBase(c).Label("思考强度：" + option.Label).Grow(1).Basis(0).Height(26).Radius(6).Center().FontSize(p.font(10.5)).TextColor(p.Text3)
					if i == int(math.Round(state.Value)) && supported {
						b.TextColor(p.AccentText).Background(p.AccentSoft)
					} else if b.Hovered() {
						b.Background(p.Hover)
					}
					b.Children(func() { ui.Text(c, option.Label).NoWrap() })
					if b.Clicked() && supported {
						state.Value = float64(i)
						changed(option.Value)
					}
				}
			})
		})
		if !supported {
			muted(c, p, "选择支持思考强度的模型后可调整").FontSize(p.font(10.36))
		}
	})
}
