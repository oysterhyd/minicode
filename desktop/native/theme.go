package main

import (
	"embed"
	"io/fs"
	"path/filepath"
	"strings"
	"time"

	"github.com/egoist/mygo/plugins/glass"
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
	// Ink fills primary buttons and switches that are on, as Claude's do:
	// near black in light mode, near white in dark mode.
	Ink, InkHover, InkText ui.Color
	// Thumb is the raised part of a segmented control or a switch.
	Thumb ui.Color
	// Prose is the font of the assistant's replies and of titles.
	Prose string
	Dark  bool
	Scale float32
}

// Fonts. The interface is set in the system's sans serif; replies, the
// greeting and page titles in a book serif, as Claude sets them, with the
// CJK text falling back to the system's UI face.
const (
	sansFont   = "Segoe UI Variable Text, Segoe UI, Microsoft YaHei UI"
	serifFont  = "Georgia, Cambria, Microsoft YaHei UI"
	systemFont = "Segoe UI, Microsoft YaHei UI"
	monoFont   = "Cascadia Mono, Cascadia Code, Consolas, Microsoft YaHei UI"
)

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
			App: h("#1f1e1d"), Main: h("#262624"), Panel: h("#262624"), Elevated: h("#30302e"), Subtle: h("#1a1918"),
			Hover: ui.RGBA(250, 249, 245, 0.05), Active: ui.RGBA(250, 249, 245, 0.085), Input: h("#30302e"),
			Border: ui.RGBA(222, 220, 209, 0.11), BorderSubtle: ui.RGBA(222, 220, 209, 0.07), BorderStrong: ui.RGBA(222, 220, 209, 0.19),
			Text: h("#faf9f5"), Text2: h("#c9c7bd"), Text3: h("#9c9a92"), Text4: h("#6e6c66"),
			Accent: h("#d97757"), AccentHover: h("#c96442"), AccentSoft: ui.RGBA(217, 119, 87, 0.16), AccentSofter: ui.RGBA(217, 119, 87, 0.09), AccentText: h("#e39272"), Selection: ui.RGBA(217, 119, 87, 0.32),
			Success: h("#86b98c"), SuccessSoft: ui.RGBA(134, 185, 140, 0.14), Danger: h("#e0897c"), DangerSoft: ui.RGBA(224, 137, 124, 0.13), Warning: h("#d7a85c"), WarningSoft: ui.RGBA(215, 168, 92, 0.14), Info: h("#9db7da"), InfoSoft: ui.RGBA(157, 183, 218, 0.13),
			AddBG: ui.RGBA(134, 185, 140, 0.12), AddText: h("#9ccaa1"), AddGutter: ui.RGBA(134, 185, 140, 0.16), RemoveBG: ui.RGBA(224, 137, 124, 0.12), RemoveText: h("#eba79d"), RemoveGutter: ui.RGBA(224, 137, 124, 0.16),
			Code: h("#1f1e1d"), CodeBorder: ui.RGBA(222, 220, 209, 0.08), Terminal: h("#1a1918"), TerminalText: h("#e8e6dc"), TerminalDim: h("#9c9a92"), Overlay: ui.RGBA(0, 0, 0, 0.42),
			Ink: h("#faf9f5"), InkHover: h("#e8e6dc"), InkText: h("#141413"), Thumb: h("#45443f"), Dark: true,
		}
	}
	return palette{
		App: h("#f3f1ea"), Main: h("#faf9f5"), Panel: h("#faf9f5"), Elevated: h("#ffffff"), Subtle: h("#f0eee6"),
		Hover: ui.RGBA(31, 30, 29, 0.045), Active: ui.RGBA(31, 30, 29, 0.075), Input: h("#ffffff"),
		Border: ui.RGBA(31, 30, 29, 0.11), BorderSubtle: ui.RGBA(31, 30, 29, 0.07), BorderStrong: ui.RGBA(31, 30, 29, 0.19),
		Text: h("#141413"), Text2: h("#3d3d3a"), Text3: h("#73726c"), Text4: h("#a3a199"),
		Accent: h("#d97757"), AccentHover: h("#c6613f"), AccentSoft: ui.RGBA(217, 119, 87, 0.12), AccentSofter: ui.RGBA(217, 119, 87, 0.07), AccentText: h("#b85636"), Selection: ui.RGBA(217, 119, 87, 0.22),
		Success: h("#4c7f55"), SuccessSoft: ui.RGBA(76, 127, 85, 0.12), Danger: h("#b1493d"), DangerSoft: ui.RGBA(177, 73, 61, 0.1), Warning: h("#a8742a"), WarningSoft: ui.RGBA(168, 116, 42, 0.12), Info: h("#3b5f8a"), InfoSoft: ui.RGBA(59, 95, 138, 0.1),
		AddBG: ui.RGBA(76, 127, 85, 0.1), AddText: h("#3b6b44"), AddGutter: ui.RGBA(76, 127, 85, 0.14), RemoveBG: ui.RGBA(177, 73, 61, 0.085), RemoveText: h("#9e3f35"), RemoveGutter: ui.RGBA(177, 73, 61, 0.12),
		Code: h("#f5f4ef"), CodeBorder: ui.RGBA(31, 30, 29, 0.07), Terminal: h("#f5f4ef"), TerminalText: h("#3d3d3a"), TerminalDim: h("#73726c"), Overlay: ui.RGBA(40, 36, 30, 0.22),
		Ink: h("#141413"), InkHover: h("#3d3d3a"), InkText: h("#faf9f5"), Thumb: h("#ffffff"),
	}
}

// proseFont is the font replies are set in, as the settings choose it.
func proseFont(choice string) string {
	switch choice {
	case "sans":
		return sansFont
	case "system":
		return systemFont
	}
	return serifFont
}

func (d *desktop) theme(c *ui.Context) palette {
	dark := c.Theme().Dark
	if d.preferences.Theme != "system" {
		dark = d.preferences.Theme == "dark"
	}
	p := colors(dark)
	p.Prose = proseFont(d.preferences.ChatFont)
	t := *c.Theme()
	t.Dark = dark
	t.Background = p.App
	t.Surface = p.Elevated
	t.SurfaceHover = p.Elevated.Mix(p.Text, 0.04)
	t.SurfacePressed = p.Elevated.Mix(p.Text, 0.08)
	t.Border = p.Border
	t.Text = p.Text
	t.TextMuted = p.Text3
	t.Accent = p.Ink
	t.AccentHover = p.InkHover
	t.AccentPressed = p.InkHover
	t.AccentText = p.InkText
	t.Danger = p.Danger
	t.Warning = p.Warning
	t.Success = p.Success
	t.Selection = p.Selection
	t.Focus = p.Accent.Alpha(0.45)
	t.FontSize = 14 * d.preferences.FontScale * max(1, c.Preferences().TextScale)
	p.Scale = t.FontSize / 14
	t.Font = sansFont
	t.Radius = 10
	t.Spacing = 3
	t.Scrollbar = p.BorderStrong
	t.ScrollbarWidth = 4
	t.Inverse = p.Ink
	t.InverseText = p.InkText
	c.SetTheme(&t)
	root := c.Root().TextColor(p.Text).Font(t.Font).FontSize(t.FontSize).LineHeight(1.5)
	if c.Vibrancy() && !d.preferences.SolidSidebar {
		root.Background(ui.Transparent)
	} else {
		root.Background(p.App)
	}
	d.lastDark = dark
	appReduceMotion = d.preferences.ReduceMotion
	if d.preferences.ReduceMotion {
		c.DisableAnimations()
	}
	return p
}

func icon(c *ui.Context, name string, size float32, color ui.Color) {
	if svg := icons[name]; svg != nil {
		ui.Icon(c, svg).FontSize(size).TextColor(color).Shrink(0)
	}
}

var fastMotion = ui.ElementTransition{Colors: true, Duration: 140 * time.Millisecond}
var panelMotion = ui.ElementTransition{Size: true, Position: true, Animation: &ui.Animation{Spring: ui.SpringOptions{Duration: 420 * time.Millisecond, Bounce: 0.06}}}
var enterMotion = ui.ElementTransition{Duration: 220 * time.Millisecond, Ease: ui.CubicBezier(0.2, 0.8, 0.2, 1), Enter: &ui.Motion{Y: 8, Opacity: 0}, Exit: &ui.Motion{Y: 4, Opacity: 0}}

// styleButton dresses a button as Claude's: a quiet outline, or ink for
// the primary action, rounded as macOS rounds its push buttons.
func styleButton(e ui.Element, p palette, primary bool) ui.Element {
	e.Height(30).Padding(0, 12).Gap(6).Radius(10).FontSize(p.font(12.32)).FontWeight(550).Cursor(ui.CursorDefault)
	fill, color, border := ui.Transparent, p.Text2, p.Border
	if primary {
		fill, color, border = p.Ink, p.InkText, ui.Transparent
	}
	if e.Hovered() {
		if primary {
			fill = p.InkHover
		} else {
			fill = p.Hover
			color = p.Text
			border = p.BorderStrong
		}
	}
	e.Background(fill).TextColor(color).Border(1, border).Transition(fastMotion)
	target := float32(1)
	if e.Pressed() {
		target = 0.96
	}
	e.AnimatePose(ui.Pose{Scale: target}, pressMotion)
	return e
}

func button(c *ui.Context, p palette, label, iconName string, primary bool) ui.Element {
	label = tr(label)
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
		return p.InkText
	}
	return p.Text2
}

func iconButton(c *ui.Context, p palette, name, label string) ui.Element {
	label = tr(label)
	e := ui.ButtonBase(c).Label(label).Size(28, 28).Center().Radius(9).TextColor(p.Text3).Transition(fastMotion)
	if e.Hovered() {
		e.Background(p.Hover).TextColor(p.Text)
	}
	target := float32(1)
	if e.Pressed() {
		target = 0.9
	}
	e.AnimatePose(ui.Pose{Scale: target}, pressMotion)
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

func badge(c *ui.Context, p palette, text string, color, bg ui.Color) ui.Element {
	return ui.Text(c, tr(text)).FontSize(p.font(9.8)).FontWeight(550).Padding(2, 6).Radius(99).TextColor(color).Background(bg).SingleLine().Shrink(0)
}

func muted(c *ui.Context, p palette, text string) ui.Element {
	return ui.Text(c, tr(text)).FontSize(p.font(11.48)).TextColor(p.Text3)
}

func spinner(c *ui.Context, p palette) {
	ring(c, 14, p.Text3)
}

// glassy fills e with Liquid Glass, rounded by radius, as macOS 26 floats
// its bars, buttons and popovers; tint colors it a little, by its alpha.
func glassy(e ui.Element, radius float32, tint ui.Color) ui.Element {
	return e.Radius(radius).Material(glass.Glass{Tint: tint})
}

func formatDuration(d time.Duration) string {
	seconds := int(d.Seconds())
	if seconds < 60 {
		return fmtInt(seconds) + tr(" 秒")
	}
	return fmtInt(seconds/60) + tr(" 分 ") + fmtInt(seconds%60) + tr(" 秒")
}
