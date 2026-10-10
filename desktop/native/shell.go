package main

import (
	"fmt"
	"slices"
	"strings"
	"time"

	"github.com/egoist/mygo/ui"
	"minicode.desktop/internal/model"
)

func (d *desktop) view(c *ui.Context) {
	p := d.theme(c)
	ui.Column(c).Fill().Children(func() {
		d.titlebar(c, p)
		ui.Row(c).Grow(1).AlignItems(ui.Stretch).Padding(0, 4, 8, 0).Children(func() {
			leftWidth := d.preferences.SidebarWidth
			if d.preferences.LeftCollapsed {
				leftWidth = 44
			}
			ui.Column(c).Key("sidebar").Width(leftWidth).FillHeight().ClipX().Transition(panelMotion).Children(func() { d.sidebar(c, p) })
			if !d.preferences.LeftCollapsed {
				d.splitter(c, p, &d.preferences.SidebarWidth, 200, 400, false)
			} else {
				ui.Box(c).Width(0)
			}
			ui.Column(c).Key("main").Grow(1).Basis(0).MinWidth(360).FillHeight().Clip().Border(1, p.Border).Radius(12).Background(p.Main).
				Shadow(0, 1, 2, 0, ui.RGBA(0, 0, 0, 0.06)).Transition(panelMotion).Children(func() {
				v := d.current()
				if len(v.Items) == 0 && !v.Busy && !v.Loading {
					d.welcome(c, p)
				} else {
					d.feed(c, p)
					d.composer(c, p, false)
				}
			})
			if !d.preferences.RightCollapsed {
				d.splitter(c, p, &d.preferences.WorkWidth, 280, 680, true)
				ui.Column(c).Key("work").Width(d.preferences.WorkWidth).FillHeight().Clip().Border(1, p.Border).Radius(12).Background(p.Panel).Transition(panelMotion).Children(func() { d.workPanel(c, p) })
			} else {
				ui.Box(c).Width(4)
				ui.Column(c).Key("work-rail").Width(32).AlignItems(ui.Center).Gap(4).Children(func() {
					if iconButton(c, p, "PanelRightOpen", "展开工作区").Clicked() {
						d.preferences.RightCollapsed = false
						d.persist()
					}
					for i, entry := range workTabs {
						if iconButton(c, p, entry.Icon, "查看"+entry.Label).Clicked() {
							d.tab = i
							d.preferences.RightCollapsed = false
							d.persist()
						}
					}
				})
			}
		})
	})
	d.shortcuts(c)
	d.overlays(c, p)
	d.toastView(c, p)
}

func (d *desktop) splitter(c *ui.Context, p palette, width *float32, minWidth, maxWidth float32, right bool) {
	e := ui.Box(c).Width(8).FillHeight().Focusable().Label("调整面板宽度").Cursor(ui.CursorResizeEW)
	e.Children(func() {
		line := ui.Box(c).Width(2).FillHeight().Margin(12, 3).Radius(2)
		if e.Hovered() || e.Pressed() || e.FocusVisible() {
			line.Background(p.Accent)
		}
		line.Transition(fastMotion)
	})
	delta := float32(0)
	if dx, _, dragging := e.Dragged(); dragging {
		delta = dx
		if right {
			delta = -dx
		}
	}
	if e.Shortcut(0, ui.KeyLeft) {
		delta = -8
		if right {
			delta = 8
		}
	}
	if e.Shortcut(0, ui.KeyRight) {
		delta = 8
		if right {
			delta = -8
		}
	}
	if e.DoubleClicked() {
		if right {
			*width = 400
		} else {
			*width = 268
		}
		d.persist()
	}
	if delta != 0 {
		*width = max(minWidth, min(maxWidth, *width+delta))
		d.persist()
	}
}

func (d *desktop) titlebar(c *ui.Context, p palette) {
	v := d.current()
	bar := c.TitleBar()
	ui.Row(c).Height(40).Padding(0, bar.Right+8, 0, 12).Gap(8).DragWindow().Children(func() {
		ui.Row(c).Width(250).Gap(6).Children(func() {
			ui.Image(c, logo).Size(18, 18).Radius(5)
			ui.Text(c, "MiniCode").FontSize(p.font(11.9)).FontWeight(650).TextColor(p.Text2).Margin(0, 8, 0, 0)
		})
		ui.Row(c).Grow(1).Justify(ui.Center).Gap(6).Children(func() {
			ui.Text(c, basename(v.Workspace)).FontSize(p.font(11.9)).TextColor(p.Text3).SingleLine()
			icon(c, "ChevronRight", 12, p.Text4)
			ui.Text(c, v.title(d.sessions)).FontSize(p.font(11.9)).FontWeight(550).MaxWidth(360).SingleLine()
			status := "就绪"
			color, bg := p.Text3, p.Hover
			if v.Busy {
				status = "运行中"
				color, bg = p.AccentText, p.AccentSoft
			}
			for _, item := range v.Items {
				if item.ApprovalID != "" {
					status = "等待确认"
					color, bg = p.Warning, p.WarningSoft
					break
				}
			}
			ui.Row(c).Gap(5).Padding(2, 8).Radius(99).Background(bg).Children(func() {
				dot := ui.Box(c).Size(6, 6).Radius(99).Background(p.Success)
				if v.Busy {
					dot.Background(p.Accent).Opacity(0.4 + 0.6*dot.Loop("pulse", 1400*time.Millisecond, ui.Bounce(ui.EaseInOut)))
				}
				ui.Text(c, status).FontSize(p.font(9.8)).FontWeight(550).TextColor(color).NoWrap().Shrink(0)
			})
		})
		ui.Box(c).Width(250)
	})
}

func (d *desktop) sidebar(c *ui.Context, p palette) {
	v := d.current()
	if d.preferences.LeftCollapsed {
		ui.Column(c).Fill().AlignItems(ui.Center).Gap(4).Children(func() {
			if iconButton(c, p, "PanelLeftOpen", "展开左侧面板").Size(32, 32).Clicked() {
				d.preferences.LeftCollapsed = false
				d.persist()
			}
			if iconButton(c, p, "FolderOpen", "打开工作区").Size(34, 34).Clicked() {
				d.chooseWorkspace()
			}
			if iconButton(c, p, "Search", "搜索任务").Size(34, 34).Clicked() {
				d.preferences.LeftCollapsed = false
				d.focusSearch = true
				d.persist()
			}
			ui.Spacer(c)
			if iconButton(c, p, "Settings2", "设置").Size(34, 34).Clicked() {
				d.openSettings("general")
			}
			if iconButton(c, p, "Moon", "切换主题").Size(34, 34).Clicked() {
				d.toggleTheme()
			}
		})
		return
	}
	ui.Column(c).Fill().Children(func() {
		ui.Column(c).Padding(2, 8, 8, 10).Gap(6).Children(func() {
			ui.Row(c).FillWidth().Height(32).Children(func() {
				ui.Spacer(c)
				if iconButton(c, p, "PanelLeft", "收拢左侧面板").Clicked() {
					d.preferences.LeftCollapsed = true
					d.persist()
				}
			})
			search := ui.Row(c).Height(30).Padding(0, 10).Gap(7).Radius(8)
			if search.Hovered() || search.FocusWithin() {
				search.Background(p.Hover)
			}
			search.Children(func() {
				icon(c, "Search", 13, p.Text3)
				input := ui.TextInputBase(c, &d.search).Grow(1).Placeholder("搜索任务").Label("搜索任务").FontSize(p.font(12.04))
				if d.focusSearch {
					input.Focus()
					d.focusSearch = false
				}
				if d.search != "" && iconButton(c, p, "X", "清除任务搜索").Size(18, 18).Clicked() {
					d.search = ""
				}
			})
		})
		ui.Scroll(c).Grow(1).Padding(0, 4, 8, 10).Children(func() {
			query := strings.ToLower(strings.TrimSpace(d.search))
			matches := func(s model.Session) bool {
				return query == "" || strings.Contains(strings.ToLower(s.Title+" "+s.Workspace), query)
			}
			pinned := slices.DeleteFunc(slices.Clone(d.sessions), func(s model.Session) bool { return !s.Pinned || !matches(s) })
			if len(pinned) > 0 {
				ui.Text(c, "置顶").FontSize(p.font(10.36)).TextColor(p.Text3).Padding(6, 8)
				for _, session := range pinned {
					d.sessionRow(c, p, session, true)
				}
			}
			ui.Row(c).Height(28).Padding(0, 4, 0, 8).Margin(4, 0, 0, 0).Children(func() {
				ui.Text(c, "工作空间").FontSize(p.font(10.36)).FontWeight(600).TextColor(p.Text3)
				ui.Spacer(c)
				if iconButton(c, p, "FolderPlus", "打开工作区").Size(22, 22).Clicked() {
					d.chooseWorkspace()
				}
			})
			roots := []string{}
			if v.Workspace != "" {
				roots = append(roots, v.Workspace)
			}
			for _, session := range d.sessions {
				if !slices.Contains(roots, session.Workspace) {
					roots = append(roots, session.Workspace)
				}
			}
			for _, root := range d.recent {
				if !slices.Contains(roots, root) {
					roots = append(roots, root)
				}
			}
			for _, root := range roots {
				ui.Column(c).Key("project-" + root).Children(func() {
					row := ui.ButtonBase(c).Label("工作区 "+root).Height(30).Radius(8).Gap(7).Padding(0, 1).Justify(ui.Start)
					if row.Hovered() {
						row.Background(p.Hover)
					}
					row.Children(func() {
						chevron := "ChevronDown"
						if d.collapsedProjects[root] {
							chevron = "ChevronRight"
						}
						icon(c, chevron, 12, p.Text4)
						icon(c, "FolderOpen", 14, p.Text2)
						ui.Text(c, basename(root)).NoWrap().FontSize(p.font(12.04)).FontWeight(550)
						if root == v.Workspace {
							badge(c, p, "当前", p.AccentText, p.AccentSoft)
						}
						ui.Spacer(c)
						if row.Hovered() && iconButton(c, p, "Plus", "在此工作区新建任务").Size(22, 22).Clicked() {
							d.newSession(root)
						}
					})
					if row.Clicked() {
						d.collapsedProjects[root] = !d.collapsedProjects[root]
					}
					row.ContextMenu(func(menu *ui.Menu) {
						if menu.Item("打开工作区").Chosen() {
							d.newSession(root)
						}
						if menu.Item("在文件夹中显示").Chosen() {
							d.reveal(root, ".")
						}
						if menu.Item("从最近工作区移除").Chosen() {
							d.recent = slices.DeleteFunc(d.recent, func(item string) bool { return item == root })
							d.persist()
						}
					})
					if !d.collapsedProjects[root] {
						count := 0
						for _, session := range d.sessions {
							if session.Workspace == root && !session.Pinned && matches(session) {
								d.sessionRow(c, p, session, false)
								count++
							}
						}
						if count == 0 {
							muted(c, p, "暂无任务").Padding(4, 28).FontSize(p.font(11.2))
						}
					}
				})
			}
			if len(roots) == 0 {
				if button(c, p, "打开本地项目", "FolderOpen", false).FillWidth().Height(64).BorderStyle(ui.BorderDashed).Clicked() {
					d.chooseWorkspace()
				}
			}
		})
		ui.Row(c).Height(40).Padding(6, 8, 0, 10).Gap(4).BorderWidth(1, 0, 0, 0).BorderColor(p.BorderSubtle).Children(func() {
			if button(c, p, "设置", "Settings2", false).Grow(1).Height(32).Padding(0, 8).Gap(8).Justify(ui.Start).Border(0, ui.Transparent).Clicked() {
				d.openSettings("general")
			}
			if iconButton(c, p, "Moon", "切换主题").Clicked() {
				d.toggleTheme()
			}
		})
	})
}

func (d *desktop) sessionRow(c *ui.Context, p palette, session model.Session, pinned bool) {
	ui.Column(c).Key("session-"+session.ID).Margin(1, 0).Children(func() {
		if d.renameID == session.ID {
			input := ui.TextInput(c, &d.renameText).Label("重命名任务").Height(28).AutoFocus()
			if input.Submitted() {
				d.sessionAction("renameSession", session.ID, map[string]any{"title": d.renameText})
				d.renameID = ""
			}
			if input.Shortcut(0, ui.KeyEscape) {
				d.renameID = ""
			}
			return
		}
		row := ui.Row(c).Height(32).Radius(8).Transition(fastMotion)
		active := d.current().SessionID == session.ID
		if active {
			if p.Dark {
				row.Background(p.Active)
			} else {
				row.Background(ui.Hex("#e9e8e4"))
			}
		} else if row.Hovered() {
			row.Background(p.Hover)
		}
		row.Children(func() {
			main := ui.ButtonBase(c).Label(session.Title).Grow(1).Height(32).Gap(8).Padding(0, 6, 0, 28)
			main.Children(func() {
				name := ""
				color := p.Text3
				if pinned {
					name = "Pin"
				}
				for _, view := range d.views {
					if view.SessionID == session.ID && (view.Busy || view.Unread) {
						name = "CircleDashed"
						color = p.Accent
					}
				}
				if svg := icons[name]; svg != nil {
					ui.Icon(c, svg).FontSize(13).TextColor(color).Absolute().Left(9).Top(9)
				}
				ui.Text(c, session.Title).Grow(1).SingleLine().FontSize(p.font(12.04)).TextColor(p.Text2)
				if !row.Hovered() {
					stamp := session.UpdatedAt
					if stamp == "" {
						stamp = session.CreatedAt
					}
					ui.Text(c, relativeTime(stamp)).FontSize(p.font(10.08)).TextColor(p.Text4)
				}
			})
			if main.Clicked() {
				d.openSession(session)
			}
			if main.DoubleClicked() {
				d.renameID = session.ID
				d.renameText = session.Title
			}
			if row.Hovered() {
				iconButton(c, p, "Ellipsis", "任务操作").Size(26, 26).Menu(func(menu *ui.Menu) { d.sessionMenu(menu, session) })
			}
		})
		row.ContextMenu(func(menu *ui.Menu) { d.sessionMenu(menu, session) })
	})
}

func (d *desktop) sessionMenu(menu *ui.Menu, session model.Session) {
	if menu.Item("重命名").Chosen() {
		d.renameID = session.ID
		d.renameText = session.Title
	}
	label := "置顶"
	if session.Pinned {
		label = "取消置顶"
	}
	if menu.Item(label).Chosen() {
		d.sessionAction("pinSession", session.ID, map[string]any{"pinned": !session.Pinned})
	}
	menu.Separator()
	running := false
	for _, view := range d.views {
		if view.SessionID == session.ID && view.Busy {
			running = true
		}
	}
	if menu.Item("删除任务").Disabled(running).Chosen() {
		d.deleteID = session.ID
		d.modal = "delete"
	}
}

func (d *desktop) welcome(c *ui.Context, p palette) {
	v := d.current()
	_, height := c.Size()
	ui.Scroll(c).Fill().Children(func() {
		ui.Column(c).FillWidth().MinHeight(height - 50).Center().Children(func() {
			ui.Column(c).FillWidth().MaxWidth(720).Padding(40, 32).Margin(0, ui.Auto).Children(func() {
				ui.Text(c, "想做些什么？").FontSize(p.font(22)).FontWeight(550).TextAlign(ui.Center).Margin(0, 0, 12, 0)
				ui.Row(c).Justify(ui.Center).Margin(0, 0, 20, 0).Children(func() {
					workspace := basename(v.Workspace)
					if v.Workspace == "" {
						workspace = "选择工作区"
					}
					project := button(c, p, workspace, "FolderOpen", false).Label("选择工作区").
						Height(28).Border(0, ui.Transparent).FontSize(p.font(11.9)).FontWeight(400).Padding(0, 10)
					project.Children(func() { icon(c, "ChevronDown", 13, p.Text3) })
					if project.Clicked() {
						d.chooseWorkspace()
					}
				})
				d.composer(c, p, true)
			})
		})
	})
}

func (d *desktop) shortcuts(c *ui.Context) {
	v := d.current()
	if c.Shortcut(ui.Cmd, ui.KeyK) {
		if d.modal == "palette" {
			d.modal = ""
		} else {
			d.modal = "palette"
			d.paletteQuery = ""
		}
		return
	}
	if d.modal != "" || d.settings != nil {
		return
	}
	if c.Shortcut(ui.Cmd, ui.KeyN) {
		d.newSession(v.Workspace)
	}
	if c.Shortcut(ui.Cmd, ui.KeyComma) || c.Shortcut(ui.Cmd, ui.KeyI) {
		d.openSettings("general")
	}
	if c.Shortcut(ui.Cmd, ui.KeySlash) {
		d.modal = "shortcuts"
	}
	if c.Shortcut(ui.Cmd, ui.KeyB) {
		d.preferences.LeftCollapsed = !d.preferences.LeftCollapsed
		d.persist()
	}
	if c.Shortcut(ui.Cmd|ui.Shift, ui.KeyB) {
		d.preferences.RightCollapsed = !d.preferences.RightCollapsed
		d.persist()
	}
	if c.Shortcut(ui.Cmd|ui.Shift, ui.KeyL) {
		d.toggleTheme()
	}
	if c.Shortcut(ui.Cmd, ui.KeyL) && !v.Busy {
		v.Items = nil
		v.rebuildRows()
	}
	if c.Shortcut(ui.Cmd|ui.Shift, ui.KeyF) {
		d.preferences.LeftCollapsed = false
		d.focusSearch = true
		d.persist()
	}
	if c.Shortcut(ui.Cmd, ui.KeyPeriod) && v.Busy {
		d.cancel(v)
	}
	for i, key := range []ui.Key{ui.Key1, ui.Key2, ui.Key3, ui.Key4} {
		if c.Shortcut(ui.Cmd, key) {
			d.tab = i
			d.preview = nil
			d.preferences.RightCollapsed = false
			d.persist()
		}
	}
	offset := 0
	if c.Shortcut(ui.Cmd|ui.Shift, ui.KeyBracketLeft) {
		offset = -1
	}
	if c.Shortcut(ui.Cmd|ui.Shift, ui.KeyBracketRight) {
		offset = 1
	}
	if offset != 0 && len(d.sessions) > 0 {
		index := 0
		for i, s := range d.sessions {
			if s.ID == v.SessionID {
				index = i
				break
			}
		}
		d.openSession(d.sessions[(index+offset+len(d.sessions))%len(d.sessions)])
	}
}

func (d *desktop) toggleTheme() {
	if d.lastDark {
		d.preferences.Theme = "light"
	} else {
		d.preferences.Theme = "dark"
	}
	d.applyAppearance()
	d.persist()
}

func (d *desktop) toastView(c *ui.Context, p palette) {
	now := time.Now()
	d.toasts = slices.DeleteFunc(d.toasts, func(t toast) bool { return now.Sub(t.At) > 6*time.Second })
	if len(d.toasts) == 0 {
		return
	}
	ui.Overlay(c, func() {
		ui.Column(c).Absolute().Right(18).Bottom(18).Width(320).Gap(8).Children(func() {
			for _, entry := range d.toasts {
				ui.Column(c).Key(entry.ID).Padding(12, 14).Radius(12).Border(1, p.Border).Background(p.Elevated).Shadow(0, 8, 28, 0, ui.RGBA(0, 0, 0, 0.25)).Transition(enterMotion).Children(func() {
					ui.Row(c).Gap(8).Children(func() {
						icon(c, "CircleCheck", 15, p.Success)
						ui.Text(c, entry.Title).FontSize(p.font(12.32)).FontWeight(600).Grow(1)
						if iconButton(c, p, "X", "关闭通知").Size(20, 20).Clicked() {
							d.toasts = slices.DeleteFunc(d.toasts, func(t toast) bool { return t.ID == entry.ID })
						}
					})
					if entry.Detail != "" {
						muted(c, p, entry.Detail).Margin(5, 0, 0, 0)
					}
					if entry.SessionID != "" && button(c, p, "查看任务", "ArrowUpRight", false).Margin(6, 0, 0, 0).Clicked() {
						for _, s := range d.sessions {
							if s.ID == entry.SessionID {
								d.openSession(s)
								break
							}
						}
					}
				})
			}
		})
	})
	c.After(time.Second)
}

func (d *desktop) title() string { return fmt.Sprintf("MiniCode - %s", d.current().title(d.sessions)) }
