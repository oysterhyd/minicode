package main

import (
	"embed"
	"io/fs"
	"path/filepath"
	"strings"
	"time"

	"github.com/egoist/mygo/ui"
)

//go:embed assets
var assets embed.FS

var logo = ui.MustParseSVG(mustAsset("assets/app-mark.svg"))
var icons = loadIcons()

func mustAsset(path string) []byte {
	data, err := assets.ReadFile(path)
	if err != nil {
		panic(err)
	}
	return data
}
func loadIcons() map[string]*ui.SVG {
	result := map[string]*ui.SVG{}
	_ = fs.WalkDir(assets, "assets/icons", func(path string, entry fs.DirEntry, err error) error {
		if err == nil && !entry.IsDir() {
			result[strings.TrimSuffix(filepath.Base(path), ".svg")] = ui.MustParseSVG(mustAsset(path))
		}
		return nil
	})
	return result
}

type palette struct {
	App, Main, Panel, Elevated, Subtle, Hover, Active, Input, Border, BorderSubtle, BorderStrong                                  ui.Color
	Text, Text2, Text3, Text4, Accent, AccentHover, AccentSoft, AccentSofter, AccentText, Selection                               ui.Color
	Success, SuccessSoft, Danger, DangerSoft, Warning, WarningSoft, Info, InfoSoft                                                ui.Color
	AddBG, AddText, AddGutter, RemoveBG, RemoveText, RemoveGutter, Code, CodeBorder, Terminal, TerminalText, TerminalDim, Overlay ui.Color
	Dark                                                                                                                          bool
	Scale                                                                                                                         float32
}

func (p palette) font(size float32) float32 {
	if p.Scale <= 0 {
		return size
	}
	return size * p.Scale
}

func colors(dark bool) palette {
	h := ui.Hex
	if dark {
		return palette{
			App: h("#141416"), Main: h("#1c1c1f"), Panel: h("#19191c"), Elevated: h("#242428"), Subtle: h("#212124"), Hover: h("#29292d"), Active: h("#313136"), Input: h("#222226"),
			Border: h("#2c2c31"), BorderSubtle: h("#252529"), BorderStrong: h("#3b3b41"),
			Text: h("#ececee"), Text2: h("#b2b2b9"), Text3: h("#808088"), Text4: h("#5f5f67"),
			Accent: h("#e2703e"), AccentHover: h("#ec8454"), AccentSoft: h("#3a2419"), AccentSofter: h("#2a1d17"), AccentText: h("#f28c5c"), Selection: h("#5b3322"),
			Success: h("#4cc38a"), SuccessSoft: h("#16301f"), Danger: h("#f0716e"), DangerSoft: h("#3a1c1c"), Warning: h("#e5a44a"), WarningSoft: h("#36270f"), Info: h("#7ea4f5"), InfoSoft: h("#1c2640"),
			AddBG: h("#1a2e22"), AddText: h("#8fdcb0"), AddGutter: h("#1f3a2a"), RemoveBG: h("#351d1e"), RemoveText: h("#f3a5a3"), RemoveGutter: h("#442426"),
			Code: h("#18181b"), CodeBorder: h("#2a2a2f"), Terminal: h("#111113"), TerminalText: h("#e3e3e6"), TerminalDim: h("#8d8d95"), Overlay: h("#00000080"), Dark: true,
		}
	}
	return palette{
		App: h("#f5f5f3"), Main: h("#ffffff"), Panel: h("#fcfcfb"), Elevated: h("#ffffff"), Subtle: h("#f4f4f1"), Hover: h("#efefec"), Active: h("#e7e7e3"), Input: h("#ffffff"),
		Border: h("#e8e7e3"), BorderSubtle: h("#efeeeb"), BorderStrong: h("#d5d4ce"),
		Text: h("#1d1d20"), Text2: h("#54545b"), Text3: h("#86868d"), Text4: h("#a9a9ae"),
		Accent: h("#d0572b"), AccentHover: h("#b94b22"), AccentSoft: h("#fcede6"), AccentSofter: h("#fdf5f1"), AccentText: h("#b44720"), Selection: h("#f6d7c7"),
		Success: h("#25855a"), SuccessSoft: h("#e5f4ec"), Danger: h("#cc3b3b"), DangerSoft: h("#fcebea"), Warning: h("#b16d10"), WarningSoft: h("#fbf1dd"), Info: h("#3569d4"), InfoSoft: h("#e9effc"),
		AddBG: h("#e8f6ed"), AddText: h("#1c6b45"), AddGutter: h("#d3eedd"), RemoveBG: h("#fcecec"), RemoveText: h("#a33030"), RemoveGutter: h("#f7d8d8"),
		Code: h("#f7f7f5"), CodeBorder: h("#ecebe7"), Terminal: h("#1c1c1f"), TerminalText: h("#e3e3e6"), TerminalDim: h("#8d8d95"), Overlay: h("#18181b52"),
	}
}

func (d *desktop) theme(c *ui.Context) palette {
	dark := c.Theme().Dark
	if d.preferences.Theme != "system" {
		dark = d.preferences.Theme == "dark"
	}
	p := colors(dark)
	t := *c.Theme()
	t.Dark = dark
	t.Background = p.App
	t.Surface = p.Elevated
	t.SurfaceHover = p.Hover
	t.SurfacePressed = p.Active
	t.Border = p.Border
	t.Text = p.Text
	t.TextMuted = p.Text3
	t.Accent = p.Accent
	t.AccentHover = p.AccentHover
	t.AccentPressed = p.AccentHover
	t.AccentText = ui.Hex("#ffffff")
	t.Danger = p.Danger
	t.Warning = p.Warning
	t.Success = p.Success
	t.Selection = p.Selection
	t.Focus = p.Accent.Alpha(0.25)
	t.FontSize = 14 * d.preferences.FontScale * max(1, c.Preferences().TextScale)
	p.Scale = t.FontSize / 14
	t.Font = "Segoe UI Variable Text, Segoe UI, Microsoft YaHei UI"
	t.Radius = 8
	t.Spacing = 3
	t.Scrollbar = p.BorderStrong
	t.ScrollbarWidth = 4
	t.Inverse = p.Elevated
	t.InverseText = p.Text
	c.SetTheme(&t)
	c.Root().Background(p.App).TextColor(p.Text).Font(t.Font).FontSize(t.FontSize).LineHeight(1.5)
	d.lastDark = dark
	return p
}

func icon(c *ui.Context, name string, size float32, color ui.Color) {
	if svg := icons[name]; svg != nil {
		ui.Icon(c, svg).FontSize(size).TextColor(color).Shrink(0)
	}
}

var fastMotion = ui.ElementTransition{Colors: true, Duration: 120 * time.Millisecond}
var panelMotion = ui.ElementTransition{Size: true, Position: true, Duration: 220 * time.Millisecond, Ease: ui.EaseOut}
var enterMotion = ui.ElementTransition{Duration: 180 * time.Millisecond, Enter: &ui.Motion{Y: 6}, Exit: &ui.Motion{Y: 6}}

func styleButton(e *ui.Element, p palette, primary bool) *ui.Element {
	e.Height(30).Padding(0, 12).Gap(6).Radius(8).FontSize(p.font(12.32)).FontWeight(550).Cursor(ui.CursorDefault)
	fill, color, border := ui.Transparent, p.Text2, p.Border
	if primary {
		fill, color, border = p.Accent, ui.Hex("#ffffff"), ui.Transparent
	}
	if e.Hovered() {
		if primary {
			fill = p.AccentHover
		} else {
			fill = p.Hover
			color = p.Text
			border = p.BorderStrong
		}
	}
	e.Background(fill).TextColor(color).Border(1, border).Transition(fastMotion)
	target := float32(1)
	if e.Pressed() {
		target = 0.97
	}
	e.TransformScale(e.AnimateWith("press", target, 120*time.Millisecond, legacyEase))
	return e
}

func button(c *ui.Context, p palette, label, iconName string, primary bool) *ui.Element {
	e := styleButton(ui.ButtonBase(c).Label(label), p, primary)
	e.Children(func() {
		if iconName != "" {
			icon(c, iconName, 14, eText(p, primary))
		}
		ui.Text(c, label).NoWrap().Shrink(0)
	})
	return e
}
func eText(p palette, primary bool) ui.Color {
	if primary {
		return ui.Hex("#ffffff")
	}
	return p.Text2
}

func iconButton(c *ui.Context, p palette, name, label string) *ui.Element {
	e := ui.ButtonBase(c).Label(label).Size(28, 28).Center().Radius(8).TextColor(p.Text3).Transition(fastMotion)
	if e.Hovered() {
		e.Background(p.Hover).TextColor(p.Text)
	}
	target := float32(1)
	if e.Pressed() {
		target = 0.92
	}
	e.TransformScale(e.AnimateWith("press", target, 120*time.Millisecond, legacyEase))
	e.Children(func() {
		color := p.Text3
		if e.Hovered() {
			color = p.Text
		}
		icon(c, name, 15, color)
	})
	e.Tooltip(label)
	return e
}

func badge(c *ui.Context, p palette, text string, color, bg ui.Color) *ui.Element {
	return ui.Text(c, text).FontSize(p.font(9.8)).FontWeight(550).Padding(2, 6).Radius(99).TextColor(color).Background(bg).SingleLine()
}

func muted(c *ui.Context, p palette, text string) *ui.Element {
	return ui.Text(c, text).FontSize(p.font(11.48)).TextColor(p.Text3)
}

func spinner(c *ui.Context, p palette) {
	if svg := icons["LoaderCircle"]; svg != nil {
		e := ui.Icon(c, svg).FontSize(14).TextColor(p.Accent)
		e.Rotate(e.Loop("spin", 900*time.Millisecond, ui.Linear) * 360)
	}
}

func formatDuration(d time.Duration) string {
	seconds := int(d.Seconds())
	if seconds < 60 {
		return fmtInt(seconds) + " 秒"
	}
	return fmtInt(seconds/60) + " 分 " + fmtInt(seconds%60) + " 秒"
}
