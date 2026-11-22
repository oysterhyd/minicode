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
	App, Chrome, Main, Panel, Elevated, Subtle, Hover, Active, Selected, Input, Border, BorderSubtle, BorderStrong, BorderSpecular ui.Color
	Text, Text2, Text3, Text4, Accent, AccentHover, AccentSoft, AccentSofter, AccentText, SecondaryAccent, SecondarySoft, Selection  ui.Color
	Success, SuccessSoft, Danger, DangerSoft, Warning, WarningSoft, Info, InfoSoft                                                ui.Color
	AddBG, AddText, AddGutter, RemoveBG, RemoveText, RemoveGutter, Code, CodeBorder, Terminal, TerminalText, TerminalDim, Overlay ui.Color
	// Ink fills primary buttons and switches that are on, as Claude's do:
	// near black in light mode, near white in dark mode.
	Ink, InkHover, InkText ui.Color
	// Thumb is the raised part of a segmented control or a switch.
	Thumb ui.Color
	// Prose is the font of the assistant's replies and of titles.
	Prose  string
	Preset string
	Dark   bool
	Scale  float32
}

// Fonts. Both UI and prose default to modern sans-serif; serif remains an
// option in settings. Code, diffs, git metadata, telemetry and keycaps use mono.
const (
	sansFont   = "Inter, Segoe UI Variable Text, Segoe UI, PingFang SC, Microsoft YaHei UI"
	serifFont  = "Source Han Serif SC, Noto Serif SC, Noto Serif CJK SC, Georgia, Cambria, Songti SC, STZhongsong, STSong, SimSun, Microsoft YaHei UI"
	systemFont = "Segoe UI, Microsoft YaHei UI"
	monoFont   = "JetBrains Mono, Cascadia Code, Cascadia Mono, Consolas, Microsoft YaHei UI"
)

var themePresets = []struct{ ID, Label string }{
	{"mono-carbon", "Mono Carbon"},
	{"t3-noir", "T3 Noir"},
	{"warm-ink", "Warm Ink"},
	{"daylight-paper", "Daylight Paper"},
}

const (
	fontMicro   float32 = 10 // Tiny counters, keycaps, chart axis ticks, diff stats
	fontCaption float32 = 11 // Badges, timestamps, muted hints, section subtitles, status pills
	fontSmall   float32 = 12 // Code, diffs, secondary lists, form inputs, composer chips, compact controls
	fontBody    float32 = 13 // Sidebar items, standard buttons, settings rows, tool group headers
	fontProse   float32 = 14 // Conversation title, chat messages, composer input, markdown body, approval title
	fontSubhead float32 = 16 // Brand wordmark, H3 headings, command palette search
	fontHeading float32 = 18 // H2 markdown headings
	fontTitle   float32 = 20 // Settings / modal titles, H1 markdown headings
	fontPreview float32 = 24 // Font card specimen preview
	fontHero    float32 = 32 // Welcome greeting headline
)

func (p palette) font(size float32) float32 {
	if p.Scale <= 0 {
		return size
	}
	return size * p.Scale
}

func resolveThemePreset(themePref, darkPreset string, systemDark bool) string {
	switch themePref {
	case "mono-carbon", "t3-noir", "warm-ink", "daylight-paper":
		return themePref
	case "daylight", "light":
		return "daylight-paper"
	case "dark":
		switch darkPreset {
		case "t3-noir", "warm-ink":
			return darkPreset
		default:
			return "mono-carbon"
		}
	default:
		if systemDark {
			switch darkPreset {
			case "t3-noir", "warm-ink":
				return darkPreset
			default:
				return "mono-carbon"
			}
		}
		return "daylight-paper"
	}
}

func paletteFor(preset string) palette {
	h := ui.Hex
	switch preset {
	case "t3-noir":
		return palette{
			App: h("#0b0910"), Chrome: h("#100d17"), Main: h("#14111d"), Panel: h("#120f1a"), Elevated: h("#1c1727"), Subtle: h("#181422"),
			Hover: ui.RGBA(245, 240, 255, 0.05), Active: ui.RGBA(245, 240, 255, 0.09), Selected: ui.RGBA(192, 132, 252, 0.15), Input: h("#1a1525"),
			BorderSubtle: ui.RGBA(235, 225, 255, 0.07), Border: ui.RGBA(235, 225, 255, 0.11), BorderStrong: ui.RGBA(235, 225, 255, 0.20), BorderSpecular: ui.RGBA(255, 255, 255, 0.10),
			Text: h("#f7f4fc"), Text2: h("#ccc4dc"), Text3: h("#9086a6"), Text4: h("#5e5574"),
			Accent: h("#c084fc"), AccentHover: h("#d8b4fe"), AccentSoft: ui.RGBA(192, 132, 252, 0.16), AccentSofter: ui.RGBA(192, 132, 252, 0.08), AccentText: h("#d8b4fe"),
			SecondaryAccent: h("#f472b6"), SecondarySoft: ui.RGBA(244, 114, 182, 0.14), Selection: ui.RGBA(192, 132, 252, 0.30),
			Success: h("#34d399"), SuccessSoft: ui.RGBA(52, 211, 153, 0.13), Danger: h("#f87171"), DangerSoft: ui.RGBA(248, 113, 113, 0.14), Warning: h("#fbbf24"), WarningSoft: ui.RGBA(251, 191, 36, 0.14), Info: h("#60a5fa"), InfoSoft: ui.RGBA(96, 165, 250, 0.14),
			AddBG: ui.RGBA(52, 211, 153, 0.10), AddText: h("#6ee7b7"), AddGutter: ui.RGBA(52, 211, 153, 0.22), RemoveBG: ui.RGBA(248, 113, 113, 0.10), RemoveText: h("#fca5a5"), RemoveGutter: ui.RGBA(248, 113, 113, 0.22),
			Code: h("#0d0a13"), CodeBorder: ui.RGBA(235, 225, 255, 0.08), Terminal: h("#09070d"), TerminalText: h("#ebe5f7"), TerminalDim: h("#9086a6"), Overlay: ui.RGBA(5, 3, 10, 0.56),
			Ink: h("#f7f4fc"), InkHover: h("#ffffff"), InkText: h("#110d19"), Thumb: h("#2b233c"), Preset: "t3-noir", Dark: true,
		}
	case "warm-ink":
		return palette{
			App: h("#161514"), Chrome: h("#1c1b1a"), Main: h("#22211f"), Panel: h("#201f1d"), Elevated: h("#2b2a27"), Subtle: h("#1a1918"),
			Hover: ui.RGBA(250, 249, 245, 0.05), Active: ui.RGBA(250, 249, 245, 0.085), Selected: ui.RGBA(217, 119, 87, 0.15), Input: h("#292826"),
			BorderSubtle: ui.RGBA(222, 220, 209, 0.07), Border: ui.RGBA(222, 220, 209, 0.11), BorderStrong: ui.RGBA(222, 220, 209, 0.19), BorderSpecular: ui.RGBA(255, 252, 245, 0.08),
			Text: h("#faf9f5"), Text2: h("#c9c7bd"), Text3: h("#9c9a92"), Text4: h("#6e6c66"),
			Accent: h("#d97757"), AccentHover: h("#e3886a"), AccentSoft: ui.RGBA(217, 119, 87, 0.16), AccentSofter: ui.RGBA(217, 119, 87, 0.09), AccentText: h("#e39272"),
			SecondaryAccent: h("#86b98c"), SecondarySoft: ui.RGBA(134, 185, 140, 0.14), Selection: ui.RGBA(217, 119, 87, 0.32),
			Success: h("#86b98c"), SuccessSoft: ui.RGBA(134, 185, 140, 0.14), Danger: h("#e0897c"), DangerSoft: ui.RGBA(224, 137, 124, 0.13), Warning: h("#d7a85c"), WarningSoft: ui.RGBA(215, 168, 92, 0.14), Info: h("#9db7da"), InfoSoft: ui.RGBA(157, 183, 218, 0.13),
			AddBG: ui.RGBA(134, 185, 140, 0.12), AddText: h("#9ccaa1"), AddGutter: ui.RGBA(134, 185, 140, 0.20), RemoveBG: ui.RGBA(224, 137, 124, 0.12), RemoveText: h("#eba79d"), RemoveGutter: ui.RGBA(224, 137, 124, 0.20),
			Code: h("#191817"), CodeBorder: ui.RGBA(222, 220, 209, 0.08), Terminal: h("#141312"), TerminalText: h("#e8e6dc"), TerminalDim: h("#9c9a92"), Overlay: ui.RGBA(0, 0, 0, 0.46),
			Ink: h("#faf9f5"), InkHover: h("#e8e6dc"), InkText: h("#141413"), Thumb: h("#3c3b37"), Preset: "warm-ink", Dark: true,
		}
	case "daylight-paper":
		return palette{
			App: h("#ebe9e2"), Chrome: h("#f2f0e9"), Main: h("#faf9f6"), Panel: h("#f7f6f1"), Elevated: h("#ffffff"), Subtle: h("#efede6"),
			Hover: ui.RGBA(20, 20, 19, 0.045), Active: ui.RGBA(20, 20, 19, 0.08), Selected: ui.RGBA(208, 99, 64, 0.13), Input: h("#ffffff"),
			BorderSubtle: ui.RGBA(20, 20, 19, 0.07), Border: ui.RGBA(20, 20, 19, 0.11), BorderStrong: ui.RGBA(20, 20, 19, 0.20), BorderSpecular: ui.RGBA(255, 255, 255, 0.70),
			Text: h("#141413"), Text2: h("#3b3b38"), Text3: h("#6e6d68"), Text4: h("#9c9a92"),
			Accent: h("#d06340"), AccentHover: h("#b85232"), AccentSoft: ui.RGBA(208, 99, 64, 0.12), AccentSofter: ui.RGBA(208, 99, 64, 0.06), AccentText: h("#b85232"),
			SecondaryAccent: h("#0284c7"), SecondarySoft: ui.RGBA(2, 132, 199, 0.11), Selection: ui.RGBA(208, 99, 64, 0.22),
			Success: h("#15803d"), SuccessSoft: ui.RGBA(21, 128, 61, 0.11), Danger: h("#b91c1c"), DangerSoft: ui.RGBA(185, 28, 28, 0.10), Warning: h("#b45309"), WarningSoft: ui.RGBA(180, 83, 9, 0.12), Info: h("#1d4ed8"), InfoSoft: ui.RGBA(29, 78, 216, 0.10),
			AddBG: ui.RGBA(21, 128, 61, 0.09), AddText: h("#166534"), AddGutter: ui.RGBA(21, 128, 61, 0.20), RemoveBG: ui.RGBA(185, 28, 28, 0.08), RemoveText: h("#991b1b"), RemoveGutter: ui.RGBA(185, 28, 28, 0.18),
			Code: h("#f3f2ec"), CodeBorder: ui.RGBA(20, 20, 19, 0.08), Terminal: h("#16171a"), TerminalText: h("#e5e7eb"), TerminalDim: h("#9ca3af"), Overlay: ui.RGBA(20, 20, 19, 0.24),
			Ink: h("#141413"), InkHover: h("#2c2c2a"), InkText: h("#faf9f5"), Thumb: h("#ffffff"), Preset: "daylight-paper",
		}
	default: // "mono-carbon"
		return palette{
			App: h("#090a0d"), Chrome: h("#0d0f13"), Main: h("#111318"), Panel: h("#101217"), Elevated: h("#171a21"), Subtle: h("#14171d"),
			Hover: ui.RGBA(255, 255, 255, 0.045), Active: ui.RGBA(255, 255, 255, 0.08), Selected: ui.RGBA(229, 112, 74, 0.13), Input: h("#14171e"),
			BorderSubtle: ui.RGBA(255, 255, 255, 0.065), Border: ui.RGBA(255, 255, 255, 0.10), BorderStrong: ui.RGBA(255, 255, 255, 0.18), BorderSpecular: ui.RGBA(255, 255, 255, 0.09),
			Text: h("#f3f4f6"), Text2: h("#c2c6d0"), Text3: h("#878d9c"), Text4: h("#565c6b"),
			Accent: h("#e5704a"), AccentHover: h("#f0805c"), AccentSoft: ui.RGBA(229, 112, 74, 0.15), AccentSofter: ui.RGBA(229, 112, 74, 0.08), AccentText: h("#f39778"),
			SecondaryAccent: h("#38bdf8"), SecondarySoft: ui.RGBA(56, 189, 248, 0.13), Selection: ui.RGBA(229, 112, 74, 0.32),
			Success: h("#34d399"), SuccessSoft: ui.RGBA(52, 211, 153, 0.13), Danger: h("#f87171"), DangerSoft: ui.RGBA(248, 113, 113, 0.14), Warning: h("#fbbf24"), WarningSoft: ui.RGBA(251, 191, 36, 0.14), Info: h("#60a5fa"), InfoSoft: ui.RGBA(96, 165, 250, 0.14),
			AddBG: ui.RGBA(52, 211, 153, 0.10), AddText: h("#6ee7b7"), AddGutter: ui.RGBA(52, 211, 153, 0.22), RemoveBG: ui.RGBA(248, 113, 113, 0.10), RemoveText: h("#fca5a5"), RemoveGutter: ui.RGBA(248, 113, 113, 0.22),
			Code: h("#0b0d11"), CodeBorder: ui.RGBA(255, 255, 255, 0.075), Terminal: h("#08090c"), TerminalText: h("#e5e7eb"), TerminalDim: h("#878d9c"), Overlay: ui.RGBA(0, 0, 0, 0.52),
			Ink: h("#f3f4f6"), InkHover: h("#ffffff"), InkText: h("#0b0c0f"), Thumb: h("#242936"), Preset: "mono-carbon", Dark: true,
		}
	}
}

func colors(dark bool) palette {
	if dark {
		return paletteFor("mono-carbon")
	}
	return paletteFor("daylight-paper")
}

// proseFont is the font replies are set in, as the settings choose it.
func proseFont(choice string) string {
	switch choice {
	case "serif":
		return serifFont
	case "system":
		return systemFont
	}
	return sansFont
}

func (d *desktop) theme(c *ui.Context) palette {
	preset := resolveThemePreset(d.preferences.Theme, d.preferences.DarkPreset, c.Theme().Dark)
	p := paletteFor(preset)
	dark := p.Dark
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
	t.Radius = 8
	t.Spacing = 3
	t.Scrollbar = p.BorderStrong
	t.ScrollbarWidth = 4
	t.Inverse = p.Ink
	t.InverseText = p.InkText
	c.SetTheme(&t)
	root := c.Root().TextColor(p.Text).Font(t.Font).FontSize(t.FontSize).LineHeight(1.5)
	if c.Vibrancy() && !d.preferences.SolidSidebar && dark {
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

// addSpecularTop paints a subtle 1px top highlight edge to give engineered panels crisp elevation.
func addSpecularTop(e ui.Element, p palette, inset float32) ui.Element {
	return e.DrawOver(func(painter *ui.Painter, r ui.Rect) {
		if r.W <= inset*2 {
			return
		}
		painter.Fill(ui.Rect{X: r.X + inset, Y: r.Y + 1, W: r.W - inset*2, H: 1}, p.BorderSpecular, 0)
	})
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
	e.Height(30).Padding(0, 12).Gap(6).Radius(7).FontSize(p.font(fontBody)).FontWeight(550).Cursor(ui.CursorDefault)
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
	e := ui.ButtonBase(c).Label(label).Size(28, 28).Center().Radius(7).TextColor(p.Text3).Transition(fastMotion)
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
	return ui.Text(c, tr(text)).FontSize(p.font(fontCaption)).FontWeight(550).Padding(2, 6).Radius(6).TextColor(color).Background(bg).SingleLine().Shrink(0)
}

func muted(c *ui.Context, p palette, text string) ui.Element {
	return ui.Text(c, tr(text)).FontSize(p.font(fontCaption)).TextColor(p.Text3)
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
