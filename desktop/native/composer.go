package main

import (
	"fmt"
	"math"
	"regexp"
	"sort"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/egoist/mygo/ui"
	"minicode.desktop/internal/model"
)

type suggestion struct{ Value, Detail, Kind string }
type completionState struct {
	Items           []suggestion
	Index, From, To int
	Query           string
	Dismissed       bool
}

var mentionPattern = regexp.MustCompile(`(?:^|\s)@([^\s@]*)$`)

var permissionOptions = []struct{ Value, Label, Detail string }{
	{"default", "逐项确认", "每一次写入、编辑和命令执行都需要你批准。"},
	{"accept_edits", "自动编辑", "自动批准文件编辑，终端命令仍需确认。"},
	{"bypass", "全部允许", "自动执行所有工具，请确认当前工作区与工具可信。"},
}
var effortOptions = []struct{ Value, Label string }{
	{"off", "关闭"}, {"low", "低"}, {"medium", "中"}, {"high", "高"}, {"xhigh", "很高"}, {"max", "最大"},
}

func pathScore(path, query string) int {
	p, q := strings.ToLower(path), strings.ToLower(query)
	if q == "" {
		return 0
	}
	if p == q {
		return 10000
	}
	base := strings.ToLower(basename(path))
	if strings.HasPrefix(base, q) {
		return 5000 - len(path)
	}
	if strings.Contains(base, q) {
		return 3000 - len(path)
	}
	if strings.Contains(p, q) {
		return 1000 - len(path)
	}
	next, gaps, last := 0, 0, -1
	runes := []rune(q)
	for i, r := range p {
		if r == runes[next] {
			if last >= 0 {
				gaps += i - last - 1
			}
			last = i
			next++
			if next == len(runes) {
				return 500 - gaps - len(path)
			}
		}
	}
	return -1
}

func (d *desktop) suggestions(v *conversation, caret int) *completionState {
	runes := []rune(v.Draft)
	caret = max(0, min(caret, len(runes)))
	before := string(runes[:caret])
	var items []suggestion
	from := 0
	query := ""
	if match := mentionPattern.FindStringSubmatchIndex(before); match != nil {
		query = before[match[2]:match[3]]
		from = utf8.RuneCountInString(before[:match[2]]) - 1
		type scored struct {
			Path  string
			Score int
		}
		var paths []scored
		for _, path := range v.Files {
			score := pathScore(path, query)
			if score >= 0 || query == "" {
				paths = append(paths, scored{path, score})
			}
		}
		sort.SliceStable(paths, func(i, j int) bool {
			if paths[i].Score == paths[j].Score {
				return len(paths[i].Path) < len(paths[j].Path)
			}
			return paths[i].Score > paths[j].Score
		})
		for _, path := range paths[:min(8, len(paths))] {
			items = append(items, suggestion{path.Path, dirname(path.Path), "file"})
		}
	} else if strings.HasPrefix(v.Draft, "/") && !strings.Contains(v.Draft, "\n") {
		verb, tail, hasSpace := strings.Cut(v.Draft, " ")
		verb = strings.ToLower(verb)
		query = v.Draft
		if hasSpace || isSubmenu(verb) {
			switch verb {
			case "/model":
				for _, m := range d.models {
					detail := m.Provider
					if !m.Available {
						detail = "未配置"
					}
					items = append(items, suggestion{m.ID, detail, "value"})
				}
			case "/effort":
				for _, e := range effortOptions {
					items = append(items, suggestion{e.Value, "思考强度", "value"})
				}
			case "/permissions":
				for _, p := range permissionOptions {
					items = append(items, suggestion{p.Value, p.Label, "value"})
				}
			case "/resume":
				for _, s := range d.sessions[:min(30, len(d.sessions))] {
					items = append(items, suggestion{s.ID[:min(8, len(s.ID))], s.Title, "value"})
				}
			case "/skill":
				for _, s := range v.Capabilities.Skills {
					items = append(items, suggestion{s.Name, s.Description, "value"})
				}
			}
			items = filterSuggestions(items, tail)
		} else {
			for _, command := range d.commands {
				if strings.HasPrefix(strings.ToLower(command.Name), verb) {
					items = append(items, suggestion{command.Name, command.Summary, "command"})
				}
			}
		}
	}
	if len(items) == 0 {
		return nil
	}
	if len(items) > 10 {
		items = items[:10]
	}
	state := &completionState{Items: items, From: from, To: caret, Query: query}
	if d.completion != nil && d.completion.Query == query {
		state.Index = min(d.completion.Index, len(items)-1)
		state.Dismissed = d.completion.Dismissed
	}
	return state
}

func filterSuggestions(items []suggestion, query string) []suggestion {
	result := make([]suggestion, 0, len(items))
	q := strings.ToLower(query)
	for _, item := range items {
		if strings.Contains(strings.ToLower(item.Value+" "+item.Detail), q) {
			result = append(result, item)
		}
	}
	return result[:min(8, len(result))]
}
func isSubmenu(verb string) bool {
	switch verb {
	case "/model", "/effort", "/permissions", "/resume", "/skill":
		return true
	}
	return false
}

func (d *desktop) pickSuggestion(input *ui.Element, index int) {
	state := d.completion
	if state == nil || index < 0 || index >= len(state.Items) {
		return
	}
	v := d.current()
	item := state.Items[index]
	if item.Kind == "file" {
		insert := "@" + item.Value + " "
		v.Draft = input.ReplaceSelection(state.From, state.To, insert)
	} else if item.Kind == "value" {
		verb, _, _ := strings.Cut(v.Draft, " ")
		v.Draft = verb + " " + item.Value
		d.completion = nil
		d.submit()
		return
	} else if isSubmenu(item.Value) {
		v.Draft = input.ReplaceSelection(0, len([]rune(v.Draft)), item.Value+" ")
	} else {
		v.Draft = item.Value
		d.completion = nil
		d.submit()
		return
	}
	d.completion = nil
	d.focusPrompt = true
}

func (d *desktop) composer(c *ui.Context, p palette, home bool) {
	v := d.current()
	ui.Column(c).Key("composer-area").FillWidth().MaxWidth(820).Margin(0, ui.Auto).
		Padding(func() []float32 {
			if home {
				return []float32{0}
			}
			return []float32{0, 32, 18, 32}
		}()...).Gap(8).Children(func() {
		if v.Error != "" {
			ui.Row(c).Padding(8, 12).Radius(8).Border(1, p.Danger.Alpha(0.3)).Background(p.DangerSoft).Gap(8).Transition(enterMotion).Children(func() {
				icon(c, "CircleAlert", 15, p.Danger)
				ui.Text(c, v.Error).Grow(1).FontSize(p.font(11.76)).MaxHeight(100).Selectable()
				if iconButton(c, p, "X", "关闭提示").Size(20, 20).Clicked() {
					v.Error = ""
				}
			})
		}
		if v.Busy && !home {
			ui.Row(c).Justify(ui.Center).Children(func() {
				ui.Row(c).Height(30).Padding(0, 4, 0, 12).Gap(8).Radius(99).Background(p.Elevated).Border(1, p.Border).Children(func() {
					spinner(c, p)
					ui.Text(c, "任务处理中").FontSize(p.font(11.2)).TextColor(p.Text2)
					ui.Text(c, formatDuration(time.Since(v.StartedAt))).FontSize(p.font(10.64)).TextColor(p.Text3)
					if button(c, p, "停止", "Square", false).Height(22).Radius(99).Padding(0, 9).FontSize(p.font(10.36)).Clicked() {
						d.cancel(v)
					}
				})
			})
			c.After(time.Second)
		}
		if len(v.Queue) > 0 {
			ui.Column(c).Gap(4).Children(func() {
				muted(c, p, fmt.Sprintf("等待执行 · %d", len(v.Queue))).FontSize(p.font(10.36))
				remove := -1
				for i, text := range v.Queue {
					ui.Row(c).Key(fmt.Sprintf("queue-%d-%s", i, text)).Height(32).Padding(0, 4, 0, 10).Gap(8).Border(1, p.BorderStrong).BorderStyle(ui.BorderDashed).Radius(8).Background(p.Panel).Transition(enterMotion).Children(func() {
						icon(c, "ListPlus", 13, p.Text3)
						ui.Text(c, text).Grow(1).SingleLine().FontSize(p.font(11.76)).TextColor(p.Text2)
						if iconButton(c, p, "X", fmt.Sprintf("取消排队消息 %d", i+1)).Size(24, 24).Clicked() {
							remove = i
						}
					})
				}
				if remove >= 0 {
					v.Queue = append(v.Queue[:remove], v.Queue[remove+1:]...)
				}
			})
		}
		composer := ui.Column(c).Border(1, p.Border).Radius(20).Background(p.Input).Shadow(0, 6, 24, -6, ui.RGBA(0, 0, 0, 0.12)).Transition(fastMotion)
		if composer.FocusWithin() {
			composer.BorderColor(p.BorderStrong).Shadow(0, 0, 0, 4, p.Accent.Alpha(0.1))
		}
		composer.Children(func() {
			placeholder := "描述任务，输入 / 使用命令，@ 引用文件"
			if v.Workspace == "" {
				placeholder = "先打开工作区，再描述你想完成的任务…"
			} else if v.Busy {
				placeholder = "追加指令，将在当前回合结束后执行…"
			}
			lines := strings.Count(v.Draft, "\n") + 1
			input := ui.TextAreaBase(c, &v.Draft).Label("任务输入").Placeholder(placeholder).FillWidth().Height(min(220, float32(lines)*21.7+26)).MinHeight(64).
				Padding(14, 18, 4, 18).FontSize(p.font(14)).LineHeight(1.55)
			if d.focusPrompt {
				input.Focus()
				d.focusPrompt = false
			}
			_, caret := input.TextSelection()
			input.ReserveTextKeys(func(mods ui.Modifiers, key ui.Key) bool {
				if key == ui.KeyEnter {
					return (d.preferences.SendKey == "enter" && mods == 0) || (d.preferences.SendKey != "enter" && mods == ui.Cmd)
				}
				if d.completion != nil && !d.completion.Dismissed && (key == ui.KeyUp || key == ui.KeyDown || key == ui.KeyTab || key == ui.KeyEscape) {
					return mods == 0
				}
				return mods == 0 && (v.Draft == "" || d.historyIndex >= 0) && (key == ui.KeyUp || key == ui.KeyDown)
			})
			if input.Changed() {
				d.historyIndex = -1
			}
			if input.Focused() && !input.Composing() {
				d.completion = d.suggestions(v, caret)
				state := d.completion
				if state != nil && !state.Dismissed {
					if input.Shortcut(0, ui.KeyDown) {
						state.Index = (state.Index + 1) % len(state.Items)
					}
					if input.Shortcut(0, ui.KeyUp) {
						state.Index = (state.Index + len(state.Items) - 1) % len(state.Items)
					}
					if input.Shortcut(0, ui.KeyTab) || input.Shortcut(0, ui.KeyEnter) {
						d.pickSuggestion(input, state.Index)
					}
					if input.Shortcut(0, ui.KeyEscape) {
						state.Dismissed = true
					}
				} else {
					send := false
					if d.preferences.SendKey == "enter" {
						send = input.Shortcut(0, ui.KeyEnter)
					} else {
						send = input.Shortcut(ui.Cmd, ui.KeyEnter)
					}
					if send {
						d.submit()
					}
					if v.Draft == "" || d.historyIndex >= 0 {
						next := d.historyIndex
						if input.Shortcut(0, ui.KeyUp) {
							next = min(len(d.history)-1, next+1)
						}
						if input.Shortcut(0, ui.KeyDown) {
							next = max(-1, next-1)
						}
						if next != d.historyIndex {
							if d.historyIndex == -1 {
								d.historyDraft = v.Draft
							}
							d.historyIndex = next
							if next < 0 {
								v.Draft = d.historyDraft
							} else {
								v.Draft = d.history[next]
							}
						}
					}
				}
			}
			ui.Row(c).Padding(6, 8, 8, 8).Gap(4).Children(func() {
				if iconButton(c, p, "AtSign", "引用文件").Size(30, 30).Radius(99).Disabled(v.Workspace == "").Clicked() {
					anchor, caret := input.TextSelection()
					v.Draft = input.ReplaceSelection(anchor, caret, "@")
					d.focusPrompt = true
				}
				d.modelMenu(c, p, home)
				d.permissionMenu(c, p, home)
				ui.Spacer(c)
				d.contextMeter(c, p, home)
				stopping := v.Busy && strings.TrimSpace(v.Draft) == ""
				ready := stopping || (strings.TrimSpace(v.Draft) != "" && (v.Workspace != "" || strings.HasPrefix(v.Draft, "/")) && !v.Loading)
				name, label := "ArrowUp", "发送任务"
				if v.Busy {
					name, label = "ListPlus", "加入队列"
				}
				if stopping {
					name, label = "Square", "停止任务"
				}
				send := ui.ButtonBase(c).Label(label).Size(32, 32).Radius(99).Center().Disabled(!ready).Transition(fastMotion)
				bg, color := p.Active, p.Text4
				if ready {
					bg, color = p.Accent, ui.Hex("#ffffff")
				}
				if stopping {
					bg, color = p.Text, p.Main
				}
				if send.Hovered() && ready && !stopping {
					bg = p.AccentHover
				}
				send.Background(bg).Children(func() { icon(c, name, 16, color) })
				if send.Clicked() {
					d.submit()
				}
			})
			if d.completion != nil && !d.completion.Dismissed {
				d.suggestionOverlay(c, p, composer, input, home)
			}
		})
		if dropped := composer.DroppedFiles(); len(dropped) > 0 {
			for _, path := range dropped {
				if relative, err := relativeFile(v.Workspace, path); err == nil {
					v.Draft += func() string {
						if v.Draft != "" && !strings.HasSuffix(v.Draft, " ") {
							return " "
						}
						return ""
					}() + "@" + relative + " "
				}
			}
			d.focusPrompt = true
		}
		if composer.FileDragOver() {
			composer.BorderColor(p.Accent).BorderStyle(ui.BorderDashed)
		}
	})
}

func (d *desktop) suggestionOverlay(c *ui.Context, p palette, anchor, input *ui.Element, home bool) {
	state := d.completion
	if state == nil {
		return
	}
	ui.Overlay(c, func() {
		at, self := ui.AnchorTopLeft, ui.AnchorBottomLeft
		if home {
			at, self = ui.AnchorBottomLeft, ui.AnchorTopLeft
		}
		ui.Column(c).AttachTo(anchor, at, self).Width(anchor.Bounds().W).Margin(8, 0).Padding(4).Radius(12).Border(1, p.Border).Background(p.Elevated).Shadow(0, 8, 24, 0, ui.RGBA(0, 0, 0, 0.2)).Transition(enterMotion).Role(ui.RoleList).Label("输入建议").Children(func() {
			label := "命令建议"
			if state.Items[0].Kind == "file" {
				label = "引用本地文件"
			}
			ui.Row(c).Padding(5, 8).Children(func() {
				muted(c, p, label).Grow(1).FontSize(p.font(10.08))
				muted(c, p, "↑↓ 选择 · Tab 插入").FontSize(p.font(10.08))
			})
			for i, item := range state.Items {
				row := ui.ButtonBase(c).Label(item.Value+" "+item.Detail).Height(32).Radius(6).Padding(0, 10).Gap(10).Role(ui.RoleListItem)
				if i == state.Index || row.Hovered() {
					row.Background(p.Hover)
				}
				row.Children(func() {
					name := "Slash"
					title := item.Value
					if item.Kind == "file" {
						name = "FileCode2"
						title = basename(item.Value)
					}
					icon(c, name, 14, p.Text3)
					ui.Text(c, title).SingleLine().MaxWidth(240).FontSize(p.font(12.04)).TextColor(p.AccentText)
					muted(c, p, item.Detail).Grow(1).SingleLine().FontSize(p.font(10.92))
					if i == state.Index {
						icon(c, "CornerDownLeft", 12, p.Text4)
					}
				})
				if row.Clicked() {
					d.pickSuggestion(input, i)
				}
			}
		})
	})
}

func (d *desktop) modelMenu(c *ui.Context, p palette, home bool) {
	v := d.current()
	label := v.State.Model
	supports := false
	for _, m := range d.models {
		if m.ID == v.State.Model {
			if m.Name != "" {
				label = m.Name
			}
			supports = m.SupportsEffort
		}
	}
	if label == "" {
		label = "切换模型"
	}
	chip := button(c, p, label, "Cpu", false).Label("切换模型").Height(30).Radius(99).Border(0, ui.Transparent).FontSize(p.font(11.2)).MaxWidth(220)
	chip.Children(func() {
		if v.State.Effort != "" && v.State.Effort != "off" {
			for _, effort := range effortOptions {
				if effort.Value == v.State.Effort {
					badge(c, p, effort.Label, p.Text2, p.Active)
				}
			}
		}
		if svg := icons["ChevronDown"]; svg != nil {
			arrow := ui.Icon(c, svg).FontSize(12).TextColor(p.Text3)
			target := float32(0)
			if d.modelOpen {
				target = 180
			}
			arrow.Rotate(arrow.Animate("open", target, 180*time.Millisecond))
		}
	})
	if chip.Clicked() {
		d.modelOpen = !d.modelOpen
	}
	ui.PopoverBase(c, chip, &d.modelOpen, func(panel *ui.Element) {
		if !home {
			panel.AttachTo(chip, ui.AnchorTopLeft, ui.AnchorBottomLeft)
		}
		panel.Width(290).Padding(4).Radius(12).Border(1, p.Border).Background(p.Elevated).Shadow(0, 8, 24, 0, ui.RGBA(0, 0, 0, 0.2))
		panel.FocusGroup(ui.Vertical)
		muted(c, p, "切换模型").Padding(5, 8).FontSize(p.font(10.08))
		for _, m := range d.models {
			name := m.Name
			if name == "" {
				name = m.ID
			}
			row := button(c, p, name, "", false).FillWidth().Height(40).Border(0, ui.Transparent).Disabled(!m.Available && m.ID != v.State.Model)
			if m.ID == v.State.Model {
				row.AutoFocus()
			}
			if row.Clicked() {
				d.modelOpen = false
				d.changeState("setModel", map[string]any{"model": m.ID})
			}
		}
		muted(c, p, "思考强度").Padding(6, 8)
		ui.Row(c).Gap(2).Children(func() {
			for _, effort := range effortOptions {
				if button(c, p, effort.Label, "", false).Padding(0, 6).FontSize(p.font(10.5)).Disabled(!supports).Clicked() {
					d.modelOpen = false
					d.changeState("setEffort", map[string]any{"effort": effort.Value})
				}
			}
		})
		if !supports {
			muted(c, p, "当前模型不支持").Padding(4, 8)
		}
	})
}

func (d *desktop) permissionMenu(c *ui.Context, p palette, home bool) {
	v := d.current()
	label := "逐项确认"
	for _, option := range permissionOptions {
		if option.Value == v.State.PermissionMode {
			label = option.Label
		}
	}
	chip := button(c, p, label, "Shield", false).Label("权限模式").Height(30).Radius(99).Border(0, ui.Transparent).FontSize(p.font(11.2))
	chip.Children(func() {
		if svg := icons["ChevronDown"]; svg != nil {
			arrow := ui.Icon(c, svg).FontSize(12).TextColor(p.Text3)
			target := float32(0)
			if d.permissionOpen {
				target = 180
			}
			arrow.Rotate(arrow.Animate("open", target, 180*time.Millisecond))
		}
	})
	if chip.Clicked() {
		d.permissionOpen = !d.permissionOpen
	}
	ui.PopoverBase(c, chip, &d.permissionOpen, func(panel *ui.Element) {
		if !home {
			panel.AttachTo(chip, ui.AnchorTopLeft, ui.AnchorBottomLeft)
		}
		panel.Width(290).Padding(4).Radius(12).Border(1, p.Border).Background(p.Elevated)
		panel.FocusGroup(ui.Vertical)
		for _, option := range permissionOptions {
			row := ui.ButtonBase(c).Label(option.Label).FillWidth().Padding(7, 8).Radius(6)
			if option.Value == v.State.PermissionMode {
				row.AutoFocus()
			}
			if row.Hovered() {
				row.Background(p.Hover)
			}
			row.Children(func() {
				ui.Column(c).Gap(2).Children(func() {
					ui.Text(c, option.Label).FontSize(p.font(12.04))
					muted(c, p, option.Detail).FontSize(p.font(10.36))
				})
			})
			if row.Clicked() {
				d.permissionOpen = false
				d.changeState("setPermissionMode", map[string]any{"mode": option.Value})
			}
		}
	})
}

func (d *desktop) contextMeter(c *ui.Context, p palette, home bool) {
	v := d.current()
	percent := 0
	if v.State.ContextWindow > 0 {
		percent = int(math.Round(min(1, float64(v.State.ContextTokens)/float64(v.State.ContextWindow)) * 100))
	}
	chip := ui.ButtonBase(c).Label("上下文窗口详情").Height(30).Radius(99).Padding(0, 6).Gap(5).TextColor(p.Text3).Transition(fastMotion)
	if chip.Hovered() {
		chip.Background(p.Hover)
	}
	chip.Children(func() {
		contextRing(c, p, percent)
		ui.Text(c, fmt.Sprintf("%d%%", percent)).FontSize(p.font(10.36)).NoWrap()
	})
	if chip.Clicked() {
		d.contextOpen = !d.contextOpen
		if d.contextOpen {
			d.refreshState(v)
		}
	}
	ui.PopoverBase(c, chip, &d.contextOpen, func(panel *ui.Element) {
		if !home {
			panel.AttachTo(chip, ui.AnchorTopRight, ui.AnchorBottomRight)
		}
		panel.Width(280).Padding(14).Radius(12).Border(1, p.Border).Background(p.Elevated)
		ui.Row(c).Children(func() {
			ui.Text(c, "上下文窗口").FontSize(p.font(11.76)).Grow(1)
			ui.Text(c, fmt.Sprintf("%s / %s", number(v.State.ContextTokens), number(v.State.ContextWindow))).FontSize(p.font(11.76)).Bold()
		})
		parts := []struct {
			Key, Label string
			Color      ui.Color
		}{{"system", "系统提示词", ui.Hex("#8b7cf6")}, {"tools", "工具定义", ui.Hex("#3aa0d8")}, {"messages", "对话消息", p.Accent}}
		ui.Row(c).Height(6).Margin(10, 0, 12, 0).Gap(2).Radius(99).Clip().Background(p.Active).Children(func() {
			for _, part := range parts {
				ui.Box(c).WidthPercent(min(100, 100*float32(v.State.Breakdown[part.Key])/float32(max(1, v.State.ContextWindow)))).FillHeight().Background(part.Color)
			}
		})
		for _, entry := range parts {
			ui.Row(c).Height(23).Gap(8).Children(func() {
				ui.Box(c).Size(7, 7).Radius(2).Background(entry.Color)
				muted(c, p, entry.Label).Grow(1).FontSize(p.font(11.2))
				ui.Text(c, number(v.State.Breakdown[entry.Key])).FontSize(p.font(11.2))
			})
		}
		usage := v.State.Usage
		known := usage != nil && usage.Available
		if usage != nil {
			value := "未报告"
			if known {
				value = fmt.Sprintf("↑ %s　↓ %s", number(usage.Input), number(usage.Output))
			}
			ui.Row(c).Margin(10, 0, 0, 0).Children(func() {
				muted(c, p, "本会话累计").Grow(1)
				ui.Text(c, value).FontSize(p.font(10.92)).FontWeight(550)
			})
		}
		metrics := []struct{ Label, Value string }{{"缓存命中率", "未报告"}, {"最近输出 TPS", "未报告"}, {"缓存读取 / 写入", "未报告"}, {"会话平均 TPS", "未报告"}}
		if known {
			if usage.Input > 0 {
				metrics[0].Value = fmt.Sprintf("%.1f%%", min(100, float64(usage.CacheRead)/float64(usage.Input)*100))
			}
			metrics[2].Value = number(usage.CacheRead) + " / " + number(usage.CacheWrite)
		}
		if stats := v.State.Statistics; stats != nil {
			if stats.LastTPS != nil {
				metrics[1].Value = fmt.Sprintf("%.1f tok/s", *stats.LastTPS)
			}
			if stats.TPS != nil {
				metrics[3].Value = fmt.Sprintf("%.1f tok/s", *stats.TPS)
			}
		}
		for pair := 0; pair < 2; pair++ {
			ui.Row(c).Margin(12, 0, 0, 0).Gap(12).Children(func() {
				for _, metric := range metrics[pair*2 : pair*2+2] {
					ui.Column(c).Grow(1).Basis(0).Gap(3).Children(func() {
						muted(c, p, metric.Label).FontSize(p.font(10.08))
						ui.Text(c, metric.Value).FontSize(p.font(10.92)).FontWeight(550)
					})
				}
			})
		}
		if percent >= 40 && button(c, p, "立即压缩上下文", "Layers", false).FillWidth().Margin(8, 0, 0, 0).Disabled(v.SessionID == "" || v.Busy).Clicked() {
			d.contextOpen = false
			d.slash("/compact", v)
		}
	})
}

func relativeFile(root, path string) (string, error) {
	resolved, err := resolveWorkspace(root, path)
	if err != nil {
		return "", err
	}
	return relativeWorkspace(root, resolved)
}
func (d *desktop) insertMention(path string) {
	v := d.current()
	if v.Draft != "" && !strings.HasSuffix(v.Draft, " ") {
		v.Draft += " "
	}
	v.Draft += "@" + path + " "
	d.focusPrompt = true
}
func (d *desktop) promptRetry() {
	v := d.current()
	for i := len(v.Items) - 1; i >= 0; i-- {
		if v.Items[i].Kind == "user" {
			d.send(v.Items[i].Text, v)
			return
		}
	}
}
func (d *desktop) pendingApproval() *model.FeedItem {
	for _, item := range d.current().Items {
		if item.ApprovalID != "" {
			return item
		}
	}
	return nil
}
