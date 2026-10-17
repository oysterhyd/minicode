package main

import "github.com/egoist/mygo/ui"

type selectOption struct {
	Value, Label string
	Disabled     bool
}

func selectField(c *ui.Context, p palette, value *string, options []selectOption) ui.Element {
	sel := ui.SelectBase(c, value)
	label := *value
	for _, option := range options {
		if option.Value == *value {
			label = option.Label
			break
		}
	}
	sel.Trigger.Height(30).Padding(0, 8, 0, 12).Radius(10).Border(1, p.Border).Background(p.Elevated).Gap(8).Shadow(0, 1, 2, 0, ui.RGBA(0, 0, 0, 0.04)).Transition(fastMotion)
	if sel.Trigger.Hovered() {
		sel.Trigger.BorderColor(p.BorderStrong)
	}
	sel.Trigger.Children(func() {
		ui.Text(c, tr(label)).Grow(1).SingleLine().FontSize(p.font(11.76))
		icon(c, "ChevronsUpDown", 13, p.Text3)
	})
	sel.Popup(func(panel ui.Element) {
		popoverMaterial(panel.Padding(5).MinWidth(sel.Trigger.Bounds().W).MaxHeight(360), p)
		for _, option := range options {
			item := sel.Item(option.Value).Label(tr(option.Label)).Padding(7, 10).Radius(10).Gap(8).Disabled(option.Disabled)
			if item.Highlighted() {
				item.Background(p.Active)
			}
			current := option.Value == *value
			item.Children(func() {
				ui.Text(c, tr(option.Label)).Grow(1).FontSize(p.font(11.76))
				if current {
					icon(c, "Check", 13, p.Text)
				}
			})
		}
	})
	return sel.Trigger
}
