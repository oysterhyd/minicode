package main

import (
	"fmt"
	"os"
	"os/user"
	"slices"
	"strings"
	"sync"
	"time"

	"github.com/egoist/mygo/plugins/glass"
	"github.com/egoist/mygo/ui"
	"minicode.desktop/internal/model"
)

func (d *desktop) view(c *ui.Context) {
	setUILanguage(d.preferences.Language)
	p := d.theme(c)
	root := ui.Column(c).Fill()
	if d.framePainted != nil {
		root.DrawOver(func(_ *ui.Painter, _ ui.Rect) {
			if d.framePainted != nil {
				d.framePainted()
			}
		})
	}
	root.Children(func() {
		d.titlebar(c, p)
		ui.Row(c).Grow(1).AlignItems(ui.Stretch).Padding(0, 8, 8, 0).Children(func() {
			leftWidth := d.preferences.SidebarWidth
			if d.preferences.LeftCollapsed {
				leftWidth = 8
			}
			ui.Column(c.Key("sidebar")).Width(leftWidth).FillHeight().ClipX().Transition(panelMotion).Children(func() {
				if !d.preferences.LeftCollapsed {
					d.sidebar(c, p)
				}
			})
			if !d.preferences.LeftCollapsed {
				d.splitter(c, p, &d.preferences.SidebarWidth, 200, 400, false)
			}
			card := ui.Column(c.Key("main")).Grow(1).Basis(0).MinWidth(360).FillHeight().Clip().Border(1, p.BorderSubtle).Radius(16).Background(p.Main).Transition(panelMotion)
			if !p.Dark {
				card.Shadow(0, 1, 3, 0, ui.RGBA(31, 30, 29, 0.05))
			}
			card.Children(func() {
				v := d.current()
				if len(v.Items) == 0 && !v.Busy && !v.Loading {
					d.welcome(c, p)
				} else {
					d.header(c, p)
					d.conversationBody(c, p)
				}
			})
			if !d.preferences.RightCollapsed {
				d.splitter(c, p, &d.preferences.WorkWidth, 280, 680, true)
				work := ui.Column(c.Key("work")).Width(d.preferences.WorkWidth).FillHeight().Clip().Border(1, p.BorderSubtle).Radius(16).Background(p.Panel).Transition(panelMotion)
				if !p.Dark {
					work.Shadow(0, 1, 3, 0, ui.RGBA(31, 30, 29, 0.05))
				}
				work.Children(func() { d.workPanel(c, p) })
			}
		})
	})
	d.shortcuts(c)
	d.overlays(c, p)
	d.toastView(c, p)
}

// conversationBody lays the feed under a floating composer: what scrolls
// fades into the card under the header and under the composer, as macOS 26
// lets content run under its bars.
func (d *desktop) conversationBody(c *ui.Context, p palette) {
	ui.Box(c.Key("body")).Grow(1).Basis(0).MinHeight(0).FillWidth().Children(func() {
		d.feed(c, p)
		ui.Box(c).Absolute().Top(0).Left(0).Right(0).Height(22).PassThrough().Material(glass.ScrollEdge{Background: p.Main})
		dock := ui.Column(c.Key("dock")).Absolute().Left(0).Right(0).Bottom(0).Padding(28, 0, 0, 0).PassThrough().
			Material(glass.ScrollEdge{Bottom: true, Background: p.Main})
		dock.Children(func() { d.composer(c, p, false) })
		d.dockHeight = dock.LayoutBounds().H
	})
}

// header is the bar over the conversation: its title, a menu of what can
// be done with it, and what it is doing now.
func (d *desktop) header(c *ui.Context, p palette) {
	v := d.current()
	ui.Row(c).Height(50).Shrink(0).Padding(0, 10, 0, 18).Gap(6).Children(func() {
		title := ui.ButtonBase(c).Label(tr("任务操作")).Height(30).Padding(0, 6, 0, 8).Gap(4).Radius(9).MaxWidth(440).Transition(fastMotion)
		if title.Hovered() {
			title.Background(p.Hover)
		}
		title.Children(func() {
			ui.Text(c, v.title(d.sessions)).SingleLine().Shrink(1).FontSize(p.font(13.3)).FontWeight(600).TextColor(p.Text)
			icon(c, "ChevronDown", 13, p.Text3)
		})
		var session *model.Session
		for i := range d.sessions {
			if d.sessions[i].ID == v.SessionID {
				session = &d.sessions[i]
			}
		}
		if session != nil {
			title.Menu(func(menu *ui.Menu) { d.sessionMenu(menu, *session) })
		}
		d.runState(c, p, v)
		ui.Spacer(c)
		name, label := "PanelRightOpen", tr("展开工作区")
		if !d.preferences.RightCollapsed {
			name, label = "PanelRightClose", tr("收拢右侧面板")
		}
		if iconButton(c, p, name, label).Size(30, 30).Clicked() {
			d.preferences.RightCollapsed = !d.preferences.RightCollapsed
			d.persist()
		}
	})
}

// runState says what the conversation is doing: running for how long, or
// waiting for the user to decide.
func (d *desktop) runState(c *ui.Context, p palette, v *conversation) {
	waiting := false
	for _, item := range v.Items {
		if item.ApprovalID != "" {
			waiting = true
			break
		}
	}
	switch {
	case waiting:
		ui.Row(c.Key("state-waiting")).Height(24).Padding(0, 9).Gap(6).Radius(99).Background(p.WarningSoft).Transition(enterMotion).Children(func() {
			pulse(c, 6, 6, 3, p.Warning, 1600*time.Millisecond, 0.45, 1)
			ui.Text(c, tr("等待你的确认")).NoWrap().FontSize(p.font(11.2)).FontWeight(550).TextColor(p.Warning)
		})
	case v.Busy:
		ui.Row(c.Key("state-running")).Height(24).Padding(0, 9, 0, 7).Gap(5).Radius(99).Background(p.Hover).Label(tr("任务处理中")).Transition(enterMotion).Children(func() {
			spark(c, 12, p.Accent, true)
			ui.Text(c, tr("运行中")+" ·").NoWrap().FontSize(p.font(11.2)).TextColor(p.Text2)
			ui.Text(c, formatDuration(time.Since(v.StartedAt))).NoWrap().FontSize(p.font(11.2)).TextColor(p.Text2)
		})
		c.After(time.Second)
	}
}

func (d *desktop) splitter(c *ui.Context, p palette, width *float32, minWidth, maxWidth float32, right bool) {
	e := ui.Box(c).Width(8).FillHeight().Focusable().Label(tr("调整面板宽度")).Cursor(ui.CursorResizeEW)
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
	ui.Row(c).Height(40).Shrink(0).Padding(0, bar.Right+8, 0, bar.Left+10).Gap(2).DragWindow().Children(func() {
		name, label := "PanelLeft", tr("收拢左侧面板")
		if d.preferences.LeftCollapsed {
			name, label = "PanelLeftOpen", tr("展开左侧面板")
		}
		if iconButton(c, p, name, label).Size(30, 30).Clicked() {
			d.preferences.LeftCollapsed = !d.preferences.LeftCollapsed
			d.persist()
		}
		if d.preferences.LeftCollapsed {
			if iconButton(c, p, "SquarePen", tr("新建对话")).Size(30, 30).Clicked() {
				d.newSession(v.Workspace)
			}
		}
		ui.Row(c).Gap(7).Padding(0, 6).Children(func() {
			ui.Image(c, logo).Size(18, 18).Radius(5)
			ui.Text(c, "MiniCode").Font(serifFont).FontSize(p.font(15)).FontWeight(600).TextColor(p.Text).NoWrap()
		})
		ui.Spacer(c)
	})
}

func (d *desktop) sidebar(c *ui.Context, p palette) {
	v := d.current()
	ui.Column(c).Fill().Children(func() {
		ui.Column(c).Padding(2, 4, 6, 10).Gap(1).Children(func() {
			create := ui.ButtonBase(c).Label(tr("新建对话")).FillWidth().Height(34).Padding(0, 8).Gap(10).Radius(10).Justify(ui.Start).Transition(fastMotion)
			if create.Hovered() {
				create.Background(p.Hover)
			}
			pressed := float32(1)
			if create.Pressed() {
				pressed = 0.97
			}
			create.AnimatePose(ui.Pose{Scale: pressed}, pressMotion)
			create.Children(func() {
				ui.Box(c).Size(22, 22).Radius(99).Background(p.Accent).Center().Children(func() { icon(c, "Plus", 14, ui.Hex("#ffffff")) })
				ui.Text(c, tr("新建对话")).NoWrap().FontSize(p.font(13)).FontWeight(550).TextColor(p.Text)
			})
			if create.Clicked() {
				d.newSession(v.Workspace)
			}
			search := ui.Row(c).Height(32).Padding(0, 8, 0, 11).Gap(9).Radius(10).Transition(fastMotion)
			if search.FocusWithin() {
				search.Background(p.Elevated).Border(1, p.BorderSubtle)
			} else if search.Hovered() {
				search.Background(p.Hover)
			}
			search.Children(func() {
				icon(c, "Search", 15, p.Text3)
				input := ui.TextInputBase(c, &d.search).Grow(1).Placeholder(tr("搜索任务")).Label(tr("搜索任务")).FontSize(p.font(12.6))
				if d.focusSearch {
					input.Focus()
					d.focusSearch = false
				}
				if d.search != "" && iconButton(c, p, "X", tr("清除任务搜索")).Size(20, 20).Clicked() {
					d.search = ""
				}
			})
		})
		ui.Scroll(c).Grow(1).Padding(4, 4, 8, 10).Children(func() {
			list := ui.Column(c.Key("session-list"))
			d.sessionListBounds = list.Bounds()
			list.Children(func() {
				if d.activeRowY >= 0 && d.activeRowSeen {
					sel := ui.Box(c.Key("selection")).Absolute().Left(0).Right(0).Height(32).Radius(10).Background(p.Active).PassThrough()
					sel.Top(sel.AnimateTo("y", d.activeRowY, ui.Animation{Spring: ui.SpringOptions{Duration: 380 * time.Millisecond, Bounce: 0.12}}))
				}
				d.activeRowSeen = false
				groups := groupSessions(d.sessions, v.Workspace, d.recent, d.search)
				if len(groups.Pinned) > 0 {
					sideHeading(c, p, tr("置顶"))
					for _, session := range groups.Pinned {
						d.sessionRow(c, p, session, true)
					}
				}
				ui.Row(c).Height(30).Padding(0, 2, 0, 8).Margin(8, 0, 0, 0).Children(func() {
					ui.Text(c, tr("工作空间")).FontSize(p.font(11.2)).FontWeight(550).TextColor(p.Text4)
					ui.Spacer(c)
					if iconButton(c, p, "FolderPlus", tr("打开工作区")).Size(24, 24).Clicked() {
						d.chooseWorkspace()
					}
				})
				for _, root := range groups.Roots {
					ui.Column(c.Key("project-" + root)).Children(func() {
						row := ui.ButtonBase(c).Label(tr("工作区 ")+root).Height(32).Radius(10).Gap(8).Padding(0, 4, 0, 8).Justify(ui.Start).Transition(fastMotion)
						if row.Hovered() {
							row.Background(p.Hover)
						}
						row.Children(func() {
							folder := "Folder"
							if !d.collapsedProjects[root] {
								folder = "FolderOpen"
							}
							icon(c, folder, 15, p.Text3)
							ui.Text(c, basename(root)).SingleLine().Shrink(1).FontSize(p.font(12.6)).FontWeight(550).TextColor(p.Text2)
							if root == v.Workspace {
								ui.Box(c).Size(5, 5).Radius(99).Background(p.Accent).Shrink(0).Tooltip(tr("当前"))
							}
							ui.Spacer(c)
							if row.Hovered() {
								if iconButton(c, p, "Plus", tr("在此工作区新建任务")).Size(24, 24).Clicked() {
									d.newSession(root)
								}
							}
							chevron := ui.Box(c).Size(14, 14).Center().Children(func() { icon(c, "ChevronDown", 12, p.Text4) })
							turn := float32(0)
							if d.collapsedProjects[root] {
								turn = -90
							}
							chevron.AnimatePose(ui.Pose{Rotate: turn}, ui.Snappy)
						})
						if row.Clicked() {
							d.collapsedProjects[root] = !d.collapsedProjects[root]
						}
						row.ContextMenu(func(menu *ui.Menu) {
							if menu.Item(tr("打开工作区")).Chosen() {
								d.newSession(root)
							}
							if menu.Item(tr("在文件夹中显示")).Chosen() {
								d.reveal(root, ".")
							}
							if menu.Item(tr("从最近工作区移除")).Chosen() {
								d.recent = slices.DeleteFunc(d.recent, func(item string) bool { return item == root })
								d.persist()
							}
						})
						if !d.collapsedProjects[root] {
							ui.Column(c).Transition(ui.ElementTransition{Duration: 220 * time.Millisecond, Ease: ui.CubicBezier(0.2, 0.8, 0.2, 1), Enter: &ui.Motion{Collapse: true}, Exit: &ui.Motion{Collapse: true}}).Children(func() {
								for _, session := range groups.Projects[root] {
									d.sessionRow(c, p, session, false)
								}
								if len(groups.Projects[root]) == 0 {
									muted(c, p, tr("暂无任务")).Padding(4, 31).FontSize(p.font(11.2)).TextColor(p.Text4)
								}
							})
						}
					})
				}
				if len(groups.Roots) == 0 {
					open := ui.ButtonBase(c).Label(tr("打开本地项目")).FillWidth().Height(76).Margin(6, 0).Radius(14).Border(1, p.BorderStrong).BorderStyle(ui.BorderDashed).Center().Gap(6).Column()
					if open.Hovered() {
						open.Background(p.Hover)
					}
					open.Children(func() {
						icon(c, "FolderOpen", 18, p.Text3)
						ui.Text(c, tr("打开本地项目")).FontSize(p.font(12.04)).TextColor(p.Text2)
					})
					if open.Clicked() {
						d.chooseWorkspace()
					}
				}
			})
			if !d.activeRowSeen {
				d.activeRowY = -1
			}
		})
		d.sidebarFooter(c, p)
	})
}

// sideHeading labels a group of the sidebar.
func sideHeading(c *ui.Context, p palette, label string) {
	ui.Text(c, label).FontSize(p.font(11.2)).FontWeight(550).TextColor(p.Text4).Padding(8, 8, 4, 8)
}

// sidebarFooter is the account of the sidebar: who is working, on how many
// services, with the appearance and the settings at hand.
func (d *desktop) sidebarFooter(c *ui.Context, p palette) {
	ui.Row(c).Height(54).Shrink(0).Padding(0, 4, 0, 10).Gap(10).Children(func() {
		name := userName()
		ui.Box(c).Size(30, 30).Radius(99).Background(p.Ink).Center().Shrink(0).Children(func() {
			ui.Text(c, strings.ToUpper(string([]rune(name)[:1]))).FontSize(p.font(12.6)).FontWeight(650).TextColor(p.InkText)
		})
		ui.Column(c).Grow(1).Basis(0).MinWidth(0).Children(func() {
			ui.Text(c, name).SingleLine().FontSize(p.font(12.6)).FontWeight(600).TextColor(p.Text)
			services := 0
			for _, service := range d.config.Services {
				if service.Enabled {
					services++
				}
			}
			ui.Text(c, trf("本地 · %d 个 AI 服务", services)).SingleLine().FontSize(p.font(10.64)).TextColor(p.Text3)
		})
		themeIcon := "Moon"
		if d.lastDark {
			themeIcon = "Sun"
		}
		if iconButton(c, p, themeIcon, tr("切换主题")).Size(30, 30).Clicked() {
			d.toggleTheme()
		}
		if iconButton(c, p, "Settings2", tr("设置")).Size(30, 30).Clicked() {
			d.openSettings("general")
		}
	})
}

// userName is who the greeting greets: the account's name, as Windows
// shows it, capitalized.
var userName = sync.OnceValue(func() string {
	name := os.Getenv("USERNAME")
	if name == "" {
		name = os.Getenv("USER")
	}
	if current, err := user.Current(); err == nil && name == "" {
		name = current.Username
	}
	if i := strings.LastIndexAny(name, `\/`); i >= 0 {
		name = name[i+1:]
	}
	if name == "" {
		return "MiniCode"
	}
	runes := []rune(name)
	return strings.ToUpper(string(runes[:1])) + string(runes[1:])
})

func (d *desktop) sessionRow(c *ui.Context, p palette, session model.Session, pinned bool) {
	ui.Column(c.Key("session-"+session.ID)).Margin(1, 0).Children(func() {
		if d.renameID == session.ID {
			input := ui.TextInput(c, &d.renameText).Label(tr("重命名任务")).Height(32).Radius(10).AutoFocus()
			if input.Submitted() {
				d.sessionAction("renameSession", session.ID, map[string]any{"title": d.renameText})
				d.renameID = ""
			}
			if input.Shortcut(0, ui.KeyEscape) {
				d.renameID = ""
			}
			return
		}
		row := ui.Row(c).Height(32).Radius(10).Transition(fastMotion)
		active := d.current().SessionID == session.ID
		if active {
			b := row.Bounds()
			if b.H > 0 && d.sessionListBounds.H > 0 {
				d.activeRowY = b.Y - d.sessionListBounds.Y
				d.activeRowSeen = true
			} else {
				row.Background(p.Active)
			}
		} else if row.Hovered() {
			row.Background(p.Hover)
		}
		row.Children(func() {
			main := ui.ButtonBase(c).Label(session.Title).Grow(1).Height(32).Gap(8).Padding(0, 8, 0, 8)
			main.Children(func() {
				busy, unread, waiting := false, false, false
				for _, view := range d.views {
					if view.SessionID == session.ID {
						busy = busy || view.Busy
						unread = unread || view.Unread
						for _, item := range view.Items {
							if item.ApprovalID != "" {
								waiting = true
							}
						}
					}
				}
				switch {
				case waiting:
					pulse(c, 7, 7, 3.5, p.Warning, 1600*time.Millisecond, 0.5, 1).Tooltip(tr("等待你的确认"))
				case busy:
					spark(c, 13, p.Accent, true)
				case unread:
					ui.Box(c).Size(7, 7).Radius(99).Background(p.Accent).Shrink(0)
				case pinned:
					icon(c, "Pin", 13, p.Text4)
				}
				color, weight := p.Text2, 400
				if active {
					color, weight = p.Text, 550
				}
				ui.Text(c, session.Title).Grow(1).SingleLine().FontSize(p.font(12.6)).FontWeight(weight).TextColor(color)
				if !row.Hovered() {
					stamp := session.UpdatedAt
					if stamp == "" {
						stamp = session.CreatedAt
					}
					ui.Text(c, relativeTime(stamp)).NoWrap().Shrink(0).FontSize(p.font(10.64)).TextColor(p.Text4)
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
				iconButton(c, p, "Ellipsis", tr("任务操作")).Size(26, 26).Menu(func(menu *ui.Menu) { d.sessionMenu(menu, session) })
			}
		})
		row.ContextMenu(func(menu *ui.Menu) { d.sessionMenu(menu, session) })
	})
}

func (d *desktop) sessionMenu(menu *ui.Menu, session model.Session) {
	if menu.Item(tr("重命名")).Chosen() {
		d.renameID = session.ID
		d.renameText = session.Title
	}
	label := tr("置顶")
	if session.Pinned {
		label = tr("取消置顶")
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
	if menu.Item(tr("删除任务")).Disabled(running).Chosen() {
		d.deleteID = session.ID
		d.modal = "delete"
	}
}

// greeting greets by the time of day, as Claude does.
func greeting(now time.Time) string {
	switch hour := now.Hour(); {
	case hour >= 5 && hour < 11:
		return tr("早上好")
	case hour >= 11 && hour < 13:
		return tr("中午好")
	case hour >= 13 && hour < 18:
		return tr("下午好")
	}
	return tr("晚上好")
}

var suggestionPrompts = []struct{ Icon, Label, Prompt string }{
	{"GitCompare", "审查改动", "审查当前工作区未提交的改动，指出问题与风险。"},
	{"Bug", "定位问题", "帮我定位并修复这个问题："},
	{"FlaskConical", "补充测试", "为最近改动的代码补充测试，并运行它们。"},
	{"BookOpen", "了解项目", "梳理这个项目的结构、入口和关键模块。"},
	{"Hammer", "实现功能", "实现以下功能："},
}

func (d *desktop) welcome(c *ui.Context, p palette) {
	v := d.current()
	_, height := c.Size()
	ui.Box(c).Fill().Children(func() {
		ui.Row(c).Absolute().Top(10).Right(10).Children(func() {
			name, label := "PanelRightOpen", tr("展开工作区")
			if !d.preferences.RightCollapsed {
				name, label = "PanelRightClose", tr("收拢右侧面板")
			}
			if iconButton(c, p, name, label).Size(30, 30).Clicked() {
				d.preferences.RightCollapsed = !d.preferences.RightCollapsed
				d.persist()
			}
		})
		ui.Scroll(c).Fill().Children(func() {
			ui.Column(c).FillWidth().MinHeight(height - 64).Center().Children(func() {
				ui.Column(c).FillWidth().MaxWidth(720).Padding(32, 32, 48, 32).Margin(0, ui.Auto).Children(func() {
					ui.Row(c).FillWidth().Justify(ui.Center).Gap(14).Margin(0, 0, 26, 0).Transition(enterMotion).Children(func() {
						spark(c, 34, p.Accent, false)
						ui.Text(c, greeting(time.Now())+tr("，")+userName()).Font(serifFont).FontSize(p.font(34)).FontWeight(400).TextColor(p.Text).LetterSpacing(-0.4).SingleLine().MinWidth(0).Shrink(1)
					})
					d.composer(c, p, true)
					ui.Row(c).Wrap().Justify(ui.Center).Gap(8).Margin(16, 0, 0, 0).Children(func() {
						for _, s := range suggestionPrompts {
							pill := ui.ButtonBase(c).Label(tr(s.Label)).Height(34).Padding(0, 14, 0, 12).Gap(7).Radius(99).Border(1, p.Border).Transition(fastMotion)
							if pill.Hovered() {
								pill.Background(p.Hover).BorderColor(p.BorderStrong)
							}
							scale := float32(1)
							if pill.Pressed() {
								scale = 0.96
							}
							pill.AnimatePose(ui.Pose{Scale: scale}, pressMotion)
							pill.Children(func() {
								icon(c, s.Icon, 14, p.Text3)
								ui.Text(c, tr(s.Label)).NoWrap().FontSize(p.font(12.32)).TextColor(p.Text2)
							})
							if pill.Clicked() {
								v.Draft = tr(s.Prompt)
								d.setCaret(len([]rune(v.Draft)))
								d.focusPrompt = true
							}
						}
					})
					d.recentTasks(c, p)
				})
			})
		})
	})
}

// recentTasks lists the last few tasks under the welcome composer.
func (d *desktop) recentTasks(c *ui.Context, p palette) {
	recent := slices.Clone(d.sessions)
	slices.SortStableFunc(recent, func(a, b model.Session) int { return strings.Compare(b.UpdatedAt, a.UpdatedAt) })
	if len(recent) == 0 {
		return
	}
	recent = recent[:min(4, len(recent))]
	ui.Column(c).Margin(36, 0, 0, 0).Gap(2).Children(func() {
		ui.Row(c).Padding(0, 10, 6, 10).Children(func() {
			ui.Text(c, tr("最近任务")).FontSize(p.font(11.76)).FontWeight(550).TextColor(p.Text3)
		})
		ui.Column(c).Gap(1).Children(func() {
			for _, session := range recent {
				row := ui.ButtonBase(c.Key("recent-"+session.ID)).Label(tr("打开任务 ")+session.Title).FillWidth().Height(38).Padding(0, 10).Gap(10).Radius(10).Justify(ui.Start).Transition(fastMotion)
				if row.Hovered() {
					row.Background(p.Hover)
				}
				row.Children(func() {
					icon(c, "MessageSquare", 14, p.Text3)
					ui.Text(c, session.Title).Grow(1).Basis(0).MinWidth(0).SingleLine().FontSize(p.font(12.6)).TextColor(p.Text2)
					ui.Text(c, basename(session.Workspace)).NoWrap().Shrink(0).FontSize(p.font(11.2)).TextColor(p.Text4)
					stamp := session.UpdatedAt
					if stamp == "" {
						stamp = session.CreatedAt
					}
					ui.Text(c, relativeTime(stamp)).NoWrap().Shrink(0).Width(52).TextAlign(ui.End).FontSize(p.font(11.2)).TextColor(p.Text4)
				})
				if row.Clicked() {
					d.openSession(session)
				}
			}
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
				toastCard := ui.Column(c.Key(entry.ID)).Padding(12, 14).Border(1, p.Border).Shadow(0, 14, 36, -8, ui.RGBA(0, 0, 0, 0.25)).Transition(enterMotion)
				glassy(toastCard, 18, p.Elevated.Alpha(0.72)).Children(func() {
					ui.Row(c).Gap(8).Children(func() {
						icon(c, "CircleCheck", 15, p.Success)
						ui.Text(c, tr(entry.Title)).FontSize(p.font(12.32)).FontWeight(600).Grow(1)
						if iconButton(c, p, "X", tr("关闭通知")).Size(20, 20).Clicked() {
							d.toasts = slices.DeleteFunc(d.toasts, func(t toast) bool { return t.ID == entry.ID })
						}
					})
					if entry.Detail != "" {
						muted(c, p, entry.Detail).Margin(5, 0, 0, 0)
					}
					if entry.SessionID != "" && button(c, p, tr("查看任务"), "ArrowUpRight", false).Margin(6, 0, 0, 0).Clicked() {
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
