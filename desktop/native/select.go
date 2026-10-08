package main

import "github.com/egoist/mygo/ui"

type selectOption struct {
	Value, Label string
	Disabled     bool
}

func selectField(c *ui.Context, p palette, value *string, options []selectOption) *ui.Element {
	sel := ui.SelectBase(c, value)
	label := *value
	for _, option := range options {
		if option.Value == *value {
			label = option.Label
			break
		}
	}
	sel.Trigger.Height(32).Padding(0, 10).Radius(8).Border(1, p.Border).Background(p.Main).Gap(8).Children(func() {
		ui.Text(c, label).Grow(1).SingleLine().FontSize(p.font(11.76))
		icon(c, "ChevronDown", 13, p.Text3)
	})
	sel.Popup(func(panel *ui.Element) {
		panel.Padding(4).MinWidth(sel.Trigger.Bounds().W).MaxHeight(360).Radius(8).Border(1, p.Border).Background(p.Elevated).Shadow(0, 8, 24, 0, ui.RGBA(0, 0, 0, 0.2))
		for _, option := range options {
			item := sel.Item(option.Value).Label(option.Label).Padding(8, 10).Radius(6).Disabled(option.Disabled)
			if item.Highlighted() {
				item.Background(p.Hover)
			}
			item.Children(func() { ui.Text(c, option.Label).FontSize(p.font(11.76)) })
		}
	})
	return sel.Trigger
}
