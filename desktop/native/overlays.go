package main

import (
	"strings"

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
	ui.DialogBase(c, &open, func(back, panel *ui.Element) {
		back.Material(modalBackdrop{p.Overlay})
		panel.Width(640).MaxWidthPercent(90).MaxHeightPercent(90).Padding(0).Radius(16).Clip().Background(p.Elevated).Border(1, p.Border).Shadow(0, 24, 64, 0, ui.RGBA(0, 0, 0, 0.3))
		switch d.modal {
		case "palette":
			d.commandPalette(c, p, panel)
		case "shortcuts":
			ui.Row(c).Height(56).Padding(0, 18).Children(func() {
				ui.Text(c, "键盘快捷键").FontSize(p.font(16)).FontWeight(650).Grow(1)
				if iconButton(c, p, "X", "关闭快捷键").Clicked() {
					open = false
				}
			})
			ui.Scroll(c).MaxHeight(500).Padding(0, 20, 20, 20).Children(func() { shortcutList(c, p) })
		case "delete":
			panel.Width(440).Padding(24).Gap(16).Label("删除任务")
			ui.Text(c, "删除任务？").FontSize(p.font(18)).FontWeight(650)
			ui.Text(c, "对话记录、运行事件和归档输出将被永久删除，工作区中的文件不受影响。").FontSize(p.font(12.6)).TextColor(p.Text2)
			ui.Row(c).Justify(ui.End).Gap(8).Children(func() {
				if button(c, p, "取消", "", false).Clicked() {
					open = false
				}
				if button(c, p, "删除", "Trash2", true).Background(p.Danger).Clicked() {
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

func (d *desktop) commandPalette(c *ui.Context, p palette, panel *ui.Element) {
	v := d.current()
	actions := []paletteAction{
		{"操作", "新建任务", "Ctrl N", "SquarePen", func() { d.newSession(v.Workspace) }},
		{"操作", "打开工作区", "本地目录", "FolderOpen", d.chooseWorkspace},
		{"操作", "打开设置", "Ctrl ,", "Settings2", func() { d.openSettings("general") }},
		{"操作", "查看键盘快捷键", "Ctrl /", "Keyboard", func() { d.modal = "shortcuts" }},
		{"操作", "切换任务侧栏", "Ctrl B", "PanelLeft", func() { d.preferences.LeftCollapsed = !d.preferences.LeftCollapsed; d.persist() }},
		{"操作", "切换工作台", "Ctrl Shift B", "PanelRight", func() { d.preferences.RightCollapsed = !d.preferences.RightCollapsed; d.persist() }},
		{"操作", "切换深浅主题", "外观", "Moon", d.toggleTheme},
	}
	for _, session := range d.sessions {
		actions = append(actions, paletteAction{"任务", session.Title, basename(session.Workspace), "ArrowUpRight", func() { d.openSession(session) }})
	}
	if strings.TrimSpace(d.paletteQuery) != "" {
		for _, path := range v.Files {
			actions = append(actions, paletteAction{"文件", basename(path), dirname(path), "FileCode2", func() { d.insertMention(path) }})
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
	panel.Label("快捷操作")
	ui.Row(c).Height(60).Padding(0, 18).Gap(10).BorderWidth(0, 0, 1, 0).BorderColor(p.Border).Children(func() {
		icon(c, "Search", 17, p.Text3)
		input := ui.TextInputBase(c, &d.paletteQuery).Grow(1).Label("搜索操作和任务").Placeholder("搜索操作、任务或文件…").AutoFocus().FontSize(p.font(14))
		input.ReserveTextKeys(func(mods ui.Modifiers, key ui.Key) bool { return mods == 0 && (key == ui.KeyUp || key == ui.KeyDown) })
		if input.Changed() {
			d.paletteIndex = 0
		}
		if input.Shortcut(0, ui.KeyDown) && len(filtered) > 0 {
			d.paletteIndex = (d.paletteIndex + 1) % len(filtered)
		}
		if input.Shortcut(0, ui.KeyUp) && len(filtered) > 0 {
			d.paletteIndex = (d.paletteIndex + len(filtered) - 1) % len(filtered)
		}
		if input.Submitted() {
			run(d.paletteIndex)
		}
		badge(c, p, "Esc", p.Text3, p.Subtle)
	})
	ui.Scroll(c).MaxHeight(420).Padding(6).Children(func() {
		for i, action := range filtered {
			if i == 0 || filtered[i-1].Group != action.Group {
				muted(c, p, action.Group).Padding(8, 10).FontSize(p.font(10.36))
			}
			row := ui.ButtonBase(c).Label(action.Label).FillWidth().Height(38).Padding(0, 10).Gap(10).Radius(8)
			if i == d.paletteIndex || row.Hovered() {
				row.Background(p.Hover)
			}
			row.Children(func() {
				icon(c, action.Icon, 15, p.Text3)
				ui.Text(c, action.Label).Grow(1).SingleLine().FontSize(p.font(12.04))
				muted(c, p, action.Hint).SingleLine().FontSize(p.font(10.36))
			})
			if row.Clicked() {
				run(i)
			}
		}
		if len(filtered) == 0 {
			empty(c, p, "没有匹配“"+d.paletteQuery+"”的结果")
		}
	})
	ui.Row(c).Height(36).Padding(0, 16).Gap(14).BorderWidth(1, 0, 0, 0).BorderColor(p.Border).Children(func() { muted(c, p, "↑ ↓ 选择"); muted(c, p, "Enter 打开"); muted(c, p, "Esc 关闭") })
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
		ui.Column(c).Margin(12, 0).Gap(6).Children(func() {
			ui.Text(c, group.Title).FontSize(p.font(12.88)).FontWeight(650).Margin(0, 0, 4, 0)
			for _, entry := range group.Items {
				ui.Row(c).Height(30).Gap(12).Children(func() {
					ui.Text(c, entry[1]).Grow(1).FontSize(p.font(11.76)).TextColor(p.Text2)
					badge(c, p, entry[0], p.Text3, p.Subtle)
				})
			}
		})
	}
}
