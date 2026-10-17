package main

import (
	"strings"
	"time"

	"github.com/egoist/mygo/plugins/glass"
	"github.com/egoist/mygo/ui"
)

func (d *desktop) overlays(c *ui.Context, p palette) {
	if d.settings != nil {
		d.settingsView(c, p)
	}
	if d.modal == "" {
		return
	}
	open := true
	ui.DialogBase(c, &open, func(back, panel ui.Element) {
		back.Material(modalBackdrop{p.Overlay})
		panel.Width(640).MaxWidthPercent(90).MaxHeightPercent(90).Padding(0).Radius(22).Clip().Background(p.Elevated).Border(1, p.Border).Shadow(0, 30, 80, -12, ui.RGBA(0, 0, 0, 0.32)).
			Transition(ui.ElementTransition{Duration: 240 * time.Millisecond, Ease: ui.CubicBezier(0.2, 0.9, 0.25, 1), Enter: &ui.Motion{Scale: 0.96, Opacity: 0}, Exit: &ui.Motion{Scale: 0.98, Opacity: 0}})
		switch d.modal {
		case "palette":
			// Spotlight: Liquid Glass high on the window rather than a sheet.
			back.Material(modalBackdrop{p.Overlay.Alpha(0.12)})
			panel.Width(680).Radius(26).Material(glass.Glass{Tint: p.Elevated.Alpha(0.72)}).Margin(110, ui.Auto, ui.Auto, ui.Auto)
			d.commandPalette(c, p, panel)
		case "shortcuts":
			ui.Row(c).Height(60).Padding(0, 16, 0, 24).Children(func() {
				ui.Text(c, tr("键盘快捷键")).Font(serifFont).FontSize(p.font(18.2)).FontWeight(500).Grow(1)
				if iconButton(c, p, "X", tr("关闭快捷键")).Clicked() {
					open = false
				}
			})
			ui.Scroll(c).MaxHeight(520).Padding(0, 24, 24, 24).Children(func() { shortcutList(c, p) })
		case "delete":
			// An alert, as macOS centers one: icon, title, text, and two
			// buttons sharing the width.
			panel.Width(380).Padding(26, 24, 20, 24).Gap(10).AlignItems(ui.Center).Label(tr("删除任务"))
			ui.Box(c).Size(48, 48).Radius(14).Background(p.DangerSoft).Center().Margin(0, 0, 4, 0).Children(func() { icon(c, "Trash2", 22, p.Danger) })
			ui.Text(c, tr("删除任务？")).Font(serifFont).FontSize(p.font(18.2)).FontWeight(500)
			ui.Text(c, tr("对话记录、运行事件和归档输出将被永久删除，工作区中的文件不受影响。")).TextAlign(ui.Center).FontSize(p.font(12.04)).LineHeight(1.55).TextColor(p.Text3)
			ui.Row(c).FillWidth().Margin(10, 0, 0, 0).Gap(8).Children(func() {
				if button(c, p, tr("取消"), "", false).Grow(1).Basis(0).Height(34).Justify(ui.Center).Clicked() {
					open = false
				}
				if button(c, p, tr("删除"), "Trash2", true).Grow(1).Basis(0).Height(34).Justify(ui.Center).Background(p.Danger).TextColor(ui.Hex("#ffffff")).Clicked() {
					d.sessionAction("deleteSession", d.deleteID, nil)
					d.deleteID = ""
					open = false
				}
			})
		}
	})
	if !open {
		d.modal = ""
	}
}

type paletteAction struct {
	Group, Label, Hint, Icon string
	Run                      func()
}

func (d *desktop) commandPalette(c *ui.Context, p palette, panel ui.Element) {
	v := d.current()
	actions := []paletteAction{
		{tr("操作"), tr("新建任务"), "Ctrl N", "SquarePen", func() { d.newSession(v.Workspace) }},
		{tr("操作"), tr("打开工作区"), tr("本地目录"), "FolderOpen", d.chooseWorkspace},
		{tr("操作"), tr("打开设置"), "Ctrl ,", "Settings2", func() { d.openSettings("general") }},
		{tr("操作"), tr("查看键盘快捷键"), "Ctrl /", "Keyboard", func() { d.modal = "shortcuts" }},
		{tr("操作"), tr("切换任务侧栏"), "Ctrl B", "PanelLeft", func() { d.preferences.LeftCollapsed = !d.preferences.LeftCollapsed; d.persist() }},
		{tr("操作"), tr("切换工作台"), "Ctrl Shift B", "PanelRight", func() { d.preferences.RightCollapsed = !d.preferences.RightCollapsed; d.persist() }},
		{tr("操作"), tr("切换深浅主题"), tr("外观"), "Moon", d.toggleTheme},
	}
	for _, session := range d.sessions {
		actions = append(actions, paletteAction{tr("任务"), session.Title, basename(session.Workspace), "ArrowUpRight", func() { d.openSession(session) }})
	}
	if strings.TrimSpace(d.paletteQuery) != "" {
		for _, path := range v.Files {
			actions = append(actions, paletteAction{tr("文件"), basename(path), dirname(path), "FileCode2", func() { d.insertMention(path) }})
		}
	}
	filtered := make([]paletteAction, 0, len(actions))
	for _, action := range actions {
		if pathScore(action.Label+" "+action.Hint, d.paletteQuery) >= 0 {
			filtered = append(filtered, action)
		}
	}
	if len(filtered) > 25 {
		filtered = filtered[:25]
	}
	d.paletteIndex = max(0, min(d.paletteIndex, len(filtered)-1))
	run := func(index int) {
		if index >= 0 && index < len(filtered) {
			action := filtered[index]
			d.modal = ""
			action.Run()
		}
	}
	panel.Label(tr("快捷操作"))
	ui.Row(c).Height(64).Padding(0, 20).Gap(12).BorderWidth(0, 0, 1, 0).BorderColor(p.BorderSubtle).Children(func() {
		icon(c, "Search", 20, p.Text2)
		input := ui.TextInputBase(c, &d.paletteQuery).Grow(1).Label(tr("搜索操作和任务")).Placeholder(tr("搜索操作、任务或文件…")).AutoFocus().FontSize(p.font(16.8))
		count := len(filtered)
		input.OnTextKey(func(k ui.TextKey) bool {
			if k.Mods != 0 || count == 0 || (k.Key != ui.KeyUp && k.Key != ui.KeyDown) {
				return false
			}
			if k.Key == ui.KeyDown {
				d.paletteIndex = (d.paletteIndex + 1) % count
			} else {
				d.paletteIndex = (d.paletteIndex + count - 1) % count
			}
			if d.window != nil {
				d.window.Invalidate()
			}
			return true
		})
		if input.Changed() {
			d.paletteIndex = 0
		}
		if input.Submitted() {
			run(d.paletteIndex)
		}
		keycap(c, p, "Esc")
	})
	ui.Scroll(c).MaxHeight(420).Padding(8).Children(func() {
		for i, action := range filtered {
			if i == 0 || filtered[i-1].Group != action.Group {
				ui.Text(c, action.Group).Padding(10, 12, 6, 12).FontSize(p.font(10.36)).FontWeight(600).TextColor(p.Text3)
			}
			selected := i == d.paletteIndex
			row := ui.ButtonBase(c).Label(action.Label).FillWidth().Height(42).Padding(0, 10).Gap(12).Radius(12).Cursor(ui.CursorDefault)
			if selected {
				row.Background(p.Active)
			} else if row.Hovered() {
				row.Background(p.Hover)
			}
			row.Children(func() {
				ui.Box(c).Size(28, 28).Shrink(0).Radius(8).Background(p.Elevated).Border(1, p.BorderSubtle).Center().Children(func() {
					icon(c, action.Icon, 15, p.Text2)
				})
				ui.Text(c, action.Label).Grow(1).SingleLine().FontSize(p.font(12.6))
				if isShortcut(action.Hint) {
					keys(c, p, action.Hint)
				} else {
					muted(c, p, action.Hint).SingleLine().FontSize(p.font(10.64))
				}
			})
			if row.Clicked() {
				run(i)
			}
		}
		if len(filtered) == 0 {
			empty(c, p, tr("没有匹配“")+d.paletteQuery+tr("”的结果"))
		}
	})
	ui.Row(c).Height(40).Padding(0, 18).Gap(16).BorderWidth(1, 0, 0, 0).BorderColor(p.BorderSubtle).Children(func() {
		for _, hint := range [][2]string{{"↑↓", "选择"}, {"↵", "打开"}, {"Esc", "关闭"}} {
			ui.Row(c).Gap(6).Children(func() {
				keycap(c, p, hint[0])
				muted(c, p, hint[1]).FontSize(p.font(10.64))
			})
		}
	})
}

// keycap is one key, drawn as a raised chip.
func keycap(c *ui.Context, p palette, key string) ui.Element {
	return ui.Text(c, key).Font(sansFont).FontSize(p.font(10.08)).FontWeight(550).TextColor(p.Text2).MinWidth(20).TextAlign(ui.Center).
		Padding(1, 6).Radius(6).Background(p.Elevated).Border(1, p.Border).Shadow(0, 1, 0, 0, p.Border).SingleLine().Shrink(0)
}

// keys is a shortcut such as "Ctrl Shift B", a keycap per key.
func keys(c *ui.Context, p palette, shortcut string) {
	ui.Row(c).Gap(3).Shrink(0).Children(func() {
		for _, key := range strings.Fields(shortcut) {
			keycap(c, p, key)
		}
	})
}

func isShortcut(hint string) bool {
	return strings.HasPrefix(hint, "Ctrl ") || strings.HasPrefix(hint, "Cmd ")
}

var shortcutGroups = []struct {
	Title string
	Items [][2]string
}{
	{"常用", [][2]string{{"Ctrl K", "搜索操作与任务"}, {"Ctrl N", "新建任务"}, {"Ctrl ,", "打开设置"}, {"Ctrl /", "查看快捷键"}, {"Ctrl Shift F", "搜索任务"}}},
	{"任务", [][2]string{{"Enter", "发送（可改为 Ctrl Enter）"}, {"Shift Enter", "换行"}, {"Ctrl .", "停止当前任务"}, {"↑ / ↓", "在空输入框中浏览历史"}, {"/ · @", "命令与文件引用"}, {"Ctrl L", "清空当前视图"}}},
	{"审批", [][2]string{{"Y", "批准执行"}, {"A", "本会话始终允许该工具"}, {"N / Esc", "拒绝"}}},
	{"界面", [][2]string{{"Ctrl B", "收拢任务侧栏"}, {"Ctrl Shift B", "收拢工作台"}, {"Ctrl 1-4", "切换工作台标签"}, {"Ctrl Shift L", "切换深浅主题"}, {"Ctrl Shift [ / ]", "切换到上 / 下一个任务"}}},
}

func shortcutList(c *ui.Context, p palette) {
	for _, group := range shortcutGroups {
		section(c, p, group.Title, "", func() {
			for _, entry := range group.Items {
				settingsRow(c, p, entry[1], "", func() {
					if strings.Contains(entry[0], " / ") || strings.Contains(entry[0], " · ") {
						keycap(c, p, entry[0])
					} else {
						keys(c, p, entry[0])
					}
				})
			}
		})
	}
}
