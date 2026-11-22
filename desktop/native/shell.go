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
		ui.Row(c).Grow(1).AlignItems(ui.Stretch).Padding(6, 8, 8, 8).Children(func() {
			leftWidth := d.preferences.SidebarWidth
			if d.preferences.LeftCollapsed {
				leftWidth = 0
			}
			ui.Column(c.Key("sidebar")).Width(leftWidth).FillHeight().ClipX().Transition(panelMotion).Children(func() {
				if !d.preferences.LeftCollapsed {
					sideBg := p.Chrome
					if c.Vibrancy() && !d.preferences.SolidSidebar && p.Dark {
						sideBg = p.Chrome.Alpha(0.88)
					}
					sideCard := ui.Column(c).Fill().Clip().Radius(13).Border(1, p.BorderSubtle).Background(sideBg)
					addSpecularTop(sideCard, p, 13)
					sideCard.Children(func() {
						d.sidebar(c, p)
					})
				}
			})
			if !d.preferences.LeftCollapsed {
				d.splitter(c, p, &d.preferences.SidebarWidth, 200, 400, false)
			}
			card := ui.Column(c.Key("main")).Grow(1).Basis(0).MinWidth(360).FillHeight().Clip().Border(1, p.Border).Radius(13).Background(p.Main).Transition(panelMotion)
			if !p.Dark {
				card.Shadow(0, 1, 3, 0, ui.RGBA(20, 20, 19, 0.05))
			}
			addSpecularTop(card, p, 13)
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
				work := ui.Column(c.Key("work")).Width(d.preferences.WorkWidth).FillHeight().Clip().Border(1, p.Border).Radius(13).Background(p.Panel).Transition(panelMotion)
				if !p.Dark {
					work.Shadow(0, 1, 3, 0, ui.RGBA(20, 20, 19, 0.05))
				}
				addSpecularTop(work, p, 13)
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
		dock := ui.Column(c.Key("dock")).Absolute().Left(0).Right(0).Bottom(0).Padding(28, 0, 0, 0).PassThrough().
			Material(glass.ScrollEdge{Bottom: true, Background: p.Main})
		dock.Children(func() { d.composer(c, p, false) })
		d.dockHeight = dock.LayoutBounds().H
	})
}

// header is the bar over the conversation: its title, branch pill, run
// state and quick workspace actions.
func (d *desktop) header(c *ui.Context, p palette) {
	v := d.current()
	ui.Row(c).Height(48).Shrink(0).Padding(0, 10, 0, 16).Gap(8).BorderWidth(0, 0, 1, 0).BorderColor(p.BorderSubtle).Background(p.Chrome.Alpha(0.35)).Children(func() {
		title := ui.ButtonBase(c).Label(tr("任务操作")).Height(30).Padding(0, 6, 0, 8).Gap(4).Radius(7).MaxWidth(340).Shrink(1).MinWidth(0).Transition(fastMotion)
		if title.Hovered() {
			title.Background(p.Hover)
		}
		title.Children(func() {
			ui.Text(c, v.title(d.sessions)).SingleLine().Shrink(1).FontSize(p.font(fontProse)).FontWeight(600).TextColor(p.Text)
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
		if v.State.Branch != "" {
			wt := v.State.Branch
			if v.Workspace != "" {
				wt = basename(v.Workspace) + " · " + v.State.Branch
			}
			ui.Row(c).Height(22).Padding(0, 7).Gap(4).Radius(5).Background(p.Subtle).Border(1, p.BorderSubtle).Shrink(0).Children(func() {
				icon(c, "GitBranch", 11, p.Accent)
				ui.Text(c, wt).NoWrap().Font(monoFont).FontSize(p.font(fontMicro)).TextColor(p.Text2)
			})
		}
		d.runState(c, p, v)
		ui.Spacer(c)
		if len(v.Changes) > 0 {
			chg := ui.ButtonBase(c).Label(tr("提交与改动")).Height(26).Padding(0, 9).Gap(5).Radius(6).Background(p.Elevated).Border(1, p.Border).Shrink(0).Transition(fastMotion)
			if chg.Hovered() {
				chg.Background(p.Hover).BorderColor(p.BorderStrong)
			}
			chg.Children(func() {
				icon(c, "GitCompare", 12, p.Text2)
				ui.Text(c, tr("提交与改动")).NoWrap().FontSize(p.font(fontCaption)).FontWeight(550).TextColor(p.Text2)
			})
			if chg.Clicked() {
				d.tab = 0
				d.preview = nil
				d.preferences.RightCollapsed = false
				d.persist()
			}
		}
		if d.preferences.RightCollapsed && iconButton(c, p, "PanelRightOpen", tr("展开工作区")).Size(30, 30).Clicked() {
			d.preferences.RightCollapsed = false
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
		ui.Row(c.Key("state-waiting")).Height(24).Shrink(0).Padding(0, 9).Gap(6).Radius(99).Background(p.WarningSoft).Border(1, p.Warning.Alpha(0.3)).Transition(enterMotion).Children(func() {
			pulse(c, 6, 6, 3, p.Warning, 1600*time.Millisecond, 0.45, 1)
			ui.Text(c, tr("等待你的确认")).NoWrap().FontSize(p.font(fontCaption)).FontWeight(550).TextColor(p.Warning)
		})
	case v.Busy:
		ui.Row(c.Key("state-running")).Height(24).Shrink(0).Padding(0, 9, 0, 7).Gap(5).Radius(99).Background(p.AccentSoft).Border(1, p.Accent.Alpha(0.28)).Label(tr("任务处理中")).Transition(enterMotion).Children(func() {
			spark(c, 12, p.Accent, true)
			ui.Text(c, tr("运行中")+" ·").NoWrap().FontSize(p.font(fontCaption)).TextColor(p.AccentText)
			ui.Text(c, formatDuration(time.Since(v.StartedAt))).NoWrap().Font(monoFont).FontSize(p.font(fontCaption)).TextColor(p.AccentText)
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

type titlebarTab struct {
	Session               model.Session
	DraftKey              string
	Title                 string
	Active, Busy, Waiting bool
}

func (d *desktop) sessionStatus(sessionID string) (active, busy, waiting, unread bool, matched *conversation) {
	cur := d.current()
	active = sessionID != "" && cur.SessionID == sessionID
	for _, view := range d.views {
		if view.SessionID == sessionID && sessionID != "" {
			if matched == nil {
				matched = view
			}
			busy = busy || view.Busy
			unread = unread || view.Unread
			for _, item := range view.Items {
				if item.ApprovalID != "" {
					waiting = true
					break
				}
			}
		}
	}
	return
}

func (d *desktop) sessionMeta(session model.Session) (branch string, add, del int) {
	for _, view := range d.views {
		if view.SessionID == session.ID {
			if view.State.Branch != "" {
				branch = view.State.Branch
			}
			for _, ch := range view.Changes {
				add += ch.Additions
				del += ch.Deletions
			}
			if branch != "" || add+del > 0 {
				return branch, add, del
			}
		}
	}
	if cur := d.current(); cur.SessionID == session.ID {
		branch = cur.State.Branch
		for _, ch := range cur.Changes {
			add += ch.Additions
			del += ch.Deletions
		}
	}
	return branch, add, del
}

func (d *desktop) titlebarTabs() []titlebarTab {
	cur := d.current()
	limit := 4
	showDraft := cur.SessionID == "" && (len(d.sessions) == 0 || len(cur.Items) > 0 || len(d.views) > 1)
	if showDraft {
		limit = 3
	}
	selected := map[string]bool{}
	if len(d.sessions) <= limit {
		for _, s := range d.sessions {
			selected[s.ID] = true
		}
	} else {
		var urgent, normal []model.Session
		for _, s := range d.sessions {
			active, busy, waiting, _, _ := d.sessionStatus(s.ID)
			if active || waiting || busy || s.Status == "running" {
				urgent = append(urgent, s)
			} else {
				normal = append(normal, s)
			}
		}
		for _, s := range append(urgent, normal...) {
			if len(selected) >= limit {
				break
			}
			selected[s.ID] = true
		}
	}
	tabs := make([]titlebarTab, 0, limit+1)
	for _, s := range d.sessions {
		if !selected[s.ID] {
			continue
		}
		active, busy, waiting, _, _ := d.sessionStatus(s.ID)
		tabs = append(tabs, titlebarTab{
			Session: s,
			Title:   s.Title,
			Active:  active,
			Busy:    busy || s.Status == "running",
			Waiting: waiting,
		})
	}
	if showDraft {
		waiting := false
		for _, item := range cur.Items {
			if item.ApprovalID != "" {
				waiting = true
				break
			}
		}
		tabs = append(tabs, titlebarTab{
			DraftKey: cur.Key,
			Title:    cur.title(d.sessions),
			Active:   true,
			Busy:     cur.Busy,
			Waiting:  waiting,
		})
	}
	return tabs
}

func (d *desktop) telemetrySummary(v *conversation) (tpsText, cacheText, costText string) {
	tpsText, cacheText = "—", "—"
	if stats := v.State.Statistics; stats != nil {
		if stats.LastTPS != nil {
			tpsText = fmt.Sprintf("%.1f", *stats.LastTPS)
		} else if stats.TPS != nil {
			tpsText = fmt.Sprintf("%.1f", *stats.TPS)
		}
		if stats.Cost != nil {
			costText = formatUSD(*stats.Cost)
		}
	}
	if usage := v.State.Usage; usage != nil && usage.Available && usage.Input > 0 {
		pct := min(100, float64(usage.CacheRead)/float64(usage.Input)*100)
		cacheText = fmt.Sprintf("%.0f%%", pct)
	}
	return
}

func (d *desktop) titlebar(c *ui.Context, p palette) {
	v := d.current()
	bar := c.TitleBar()
	titleBg := p.Chrome
	if c.Vibrancy() && !d.preferences.SolidSidebar && p.Dark {
		titleBg = p.Chrome.Alpha(0.88)
	}
	ui.Row(c).Height(40).Shrink(0).Padding(0, bar.Right+8, 0, bar.Left+10).Gap(6).Background(titleBg).
		BorderWidth(0, 0, 1, 0).BorderColor(p.BorderSubtle).DragWindow().Children(func() {
		name, label := "PanelLeft", tr("收拢左侧面板")
		if d.preferences.LeftCollapsed {
			name, label = "PanelLeftOpen", tr("展开左侧面板")
		}
		if iconButton(c, p, name, label).Size(28, 28).Clicked() {
			d.preferences.LeftCollapsed = !d.preferences.LeftCollapsed
			d.persist()
		}
		if d.preferences.LeftCollapsed {
			if iconButton(c, p, "SquarePen", tr("新建对话")).Size(28, 28).Clicked() {
				d.newSession(v.Workspace)
			}
		}
		ui.Row(c).Gap(6).Padding(0, 4).Shrink(0).Children(func() {
			ui.Box(c).Size(20, 20).Radius(5).Gradient(p.Accent, p.Accent.Mix(ui.Hex("#000000"), 0.28), 135).Center().Shrink(0).Children(func() {
				ui.Text(c, "M").Font(monoFont).FontSize(p.font(fontCaption)).FontWeight(700).TextColor(ui.Hex("#ffffff")).NoWrap()
			})
			ui.Text(c, "MiniCode").Font(sansFont).FontSize(p.font(fontBody)).FontWeight(650).TextColor(p.Text).NoWrap().Shrink(0)
			ui.Text(c, "D3D11 · v3.0").Font(monoFont).FontSize(p.font(fontMicro)).TextColor(p.Text3).
				Padding(1, 5).Radius(4).Background(p.Hover).Border(1, p.BorderSubtle).NoWrap().Shrink(0)
		})
		tabs := d.titlebarTabs()
		ui.Row(c.Key("session-tabs")).Grow(1).Basis(0).MinWidth(0).Gap(4).Padding(0, 4).ClipX().Children(func() {
			for i, tab := range tabs {
				tab := tab
				tb := ui.ButtonBase(c.Key(fmt.Sprintf("tab-%d-%s-%s", i, tab.Session.ID, tab.DraftKey))).
					Label(trf("会话标签 %d：%s", i+1, tab.Title)).Height(28).Padding(0, 8).Gap(6).Radius(6).
					MaxWidth(210).MinWidth(72).Shrink(1).Transition(fastMotion)
				if tab.Active {
					tb.Background(p.Main).Border(1, p.Border)
				} else if tb.Hovered() {
					tb.Background(p.Hover)
				}
				tb.Children(func() {
					switch {
					case tab.Waiting:
						pulse(c, 7, 7, 3.5, p.Warning, 1400*time.Millisecond, 0.45, 1)
					case tab.Busy:
						pulse(c, 7, 7, 3.5, p.Accent, 1600*time.Millisecond, 0.45, 1)
					default:
						ui.Box(c).Size(7, 7).Radius(99).Background(p.Success).Shrink(0)
					}
					textColor, weight := p.Text3, 450
					if tab.Active {
						textColor, weight = p.Text, 550
					} else if tb.Hovered() {
						textColor = p.Text2
					}
					ui.Text(c, tab.Title).SingleLine().Shrink(1).MinWidth(0).FontSize(p.font(fontSmall)).FontWeight(weight).TextColor(textColor)
					ui.Text(c, fmt.Sprintf("⌘%d", i+1)).NoWrap().Shrink(0).Font(monoFont).FontSize(p.font(fontMicro)).TextColor(p.Text4)
				})
				if tb.Clicked() {
					if tab.Session.ID != "" {
						d.openSession(tab.Session)
					} else if tab.DraftKey != "" {
						d.activate(tab.DraftKey)
					}
				}
				if tab.Session.ID != "" {
					tb.ContextMenu(func(menu *ui.Menu) { d.sessionMenu(menu, tab.Session) })
				}
			}
			if iconButton(c, p, "Plus", tr("新建并行会话")).Size(26, 26).Radius(6).Clicked() {
				d.newSession(v.Workspace)
			}
		})
		ui.Row(c).Gap(6).Shrink(0).Children(func() {
			tpsText, cacheText, costText := d.telemetrySummary(v)
			pill := ui.ButtonBase(c).Label(tr("实时遥测")).Tooltip(tr("实时推理吞吐与会话缓存命中率")).Height(26).Padding(0, 9).Gap(5).Radius(6).
				Background(p.Subtle).Border(1, p.BorderSubtle).Shrink(0).Transition(fastMotion)
			if pill.Hovered() {
				pill.Background(p.Hover).BorderColor(p.BorderStrong)
			}
			pill.Children(func() {
				ui.Text(c, "TPS").Font(monoFont).FontSize(p.font(fontCaption)).TextColor(p.Text3).NoWrap()
				ui.Text(c, tpsText).Font(monoFont).FontSize(p.font(fontCaption)).FontWeight(600).TextColor(p.Text2).NoWrap()
				ui.Text(c, "·").Font(monoFont).FontSize(p.font(fontCaption)).TextColor(p.Text4).NoWrap()
				ui.Text(c, "Cache").Font(monoFont).FontSize(p.font(fontCaption)).TextColor(p.Text3).NoWrap()
				ui.Text(c, cacheText).Font(monoFont).FontSize(p.font(fontCaption)).FontWeight(600).TextColor(p.Text2).NoWrap()
				if costText != "" {
					ui.Text(c, "·").Font(monoFont).FontSize(p.font(fontCaption)).TextColor(p.Text4).NoWrap()
					ui.Text(c, costText).Font(monoFont).FontSize(p.font(fontCaption)).FontWeight(600).TextColor(p.Success).NoWrap()
				}
			})
			if pill.Clicked() {
				d.preferences.RightCollapsed = false
				d.tab = 3
				d.preview = nil
				d.persist()
			}
			rightIcon := "PanelRightClose"
			if d.preferences.RightCollapsed {
				rightIcon = "PanelRightOpen"
			}
			if iconButton(c, p, rightIcon, tr("切换工作台")).Size(28, 28).Clicked() {
				d.preferences.RightCollapsed = !d.preferences.RightCollapsed
				d.persist()
			}
		})
	})
}

func (d *desktop) sidebar(c *ui.Context, p palette) {
	v := d.current()
	mode := d.preferences.SidebarMode
	if mode != "tree" {
		mode = "inbox"
	}
	ui.Column(c).Fill().Children(func() {
		ui.Column(c).Padding(10, 10, 8, 10).Gap(7).BorderWidth(0, 0, 1, 0).BorderColor(p.BorderSubtle).Children(func() {
			create := ui.ButtonBase(c).Label(tr("新建对话")).FillWidth().Height(32).Padding(0, 10).Gap(8).Radius(7).
				Background(p.Ink).TextColor(p.InkText).Justify(ui.Start).Transition(fastMotion)
			if create.Hovered() {
				create.Background(p.InkHover)
			}
			pressed := float32(1)
			if create.Pressed() {
				pressed = 0.97
			}
			create.AnimatePose(ui.Pose{Scale: pressed}, pressMotion)
			create.Children(func() {
				icon(c, "Plus", 14, p.InkText)
				ui.Text(c, tr("新建对话")).Grow(1).NoWrap().FontSize(p.font(fontSmall)).FontWeight(600).TextColor(p.InkText)
				ui.Text(c, "Ctrl N").Font(monoFont).FontSize(p.font(fontMicro)).Padding(1, 5).Radius(4).
					Background(p.InkText.Alpha(0.14)).TextColor(p.InkText.Alpha(0.85)).NoWrap().Shrink(0)
			})
			if create.Clicked() {
				d.newSession(v.Workspace)
			}
			search := ui.Row(c).Height(30).Padding(0, 8, 0, 9).Gap(7).Radius(7).Background(p.Subtle).Border(1, p.BorderSubtle).Transition(fastMotion)
			if search.FocusWithin() {
				search.Background(p.Input).BorderColor(p.BorderStrong)
			} else if search.Hovered() {
				search.BorderColor(p.Border)
			}
			search.Children(func() {
				icon(c, "Search", 13, p.Text3)
				input := ui.TextInputBase(c, &d.search).Grow(1).Placeholder(tr("搜索任务")).Label(tr("搜索任务")).FontSize(p.font(fontSmall))
				if d.focusSearch {
					input.Focus()
					d.focusSearch = false
				}
				if d.search != "" && iconButton(c, p, "X", tr("清除任务搜索")).Size(20, 20).Clicked() {
					d.search = ""
				}
			})
			ui.Row(c).FillWidth().Height(28).Padding(2).Gap(2).Radius(7).Background(p.Subtle).Border(1, p.BorderSubtle).Children(func() {
				for _, tab := range []struct{ ID, Label string }{{"inbox", "Inbox 线程流"}, {"tree", "工作区目录树"}} {
					active := mode == tab.ID
					btn := ui.ButtonBase(c).Label(tr(tab.Label)).Grow(1).Basis(0).FillHeight().Radius(5).Center().Transition(fastMotion)
					if active {
						btn.Background(p.Thumb)
					} else if btn.Hovered() {
						btn.Background(p.Hover)
					}
					btn.Children(func() {
						color, weight := p.Text3, 450
						if active {
							color, weight = p.Text, 550
						}
						ui.Text(c, tr(tab.Label)).SingleLine().FontSize(p.font(fontCaption)).FontWeight(weight).TextColor(color)
					})
					if btn.Clicked() && d.preferences.SidebarMode != tab.ID {
						d.preferences.SidebarMode = tab.ID
						d.persist()
					}
				}
			})
		})
		ui.Scroll(c).Grow(1).Padding(6, 8, 8, 8).Children(func() {
			list := ui.Column(c.Key("session-list"))
			d.sessionListBounds = list.Bounds()
			list.Children(func() {
				d.activeRowSeen = false
				if mode == "inbox" {
					query := strings.ToLower(strings.TrimSpace(d.search))
					var activeSessions, settledSessions []model.Session
					for _, session := range d.sessions {
						branch, _, _ := d.sessionMeta(session)
						if query != "" && !strings.Contains(strings.ToLower(session.Title+" "+session.Workspace+" "+branch), query) {
							continue
						}
						_, busy, waiting, unread, _ := d.sessionStatus(session.ID)
						if busy || waiting || unread || session.Status == "running" || session.Status == "paused" {
							activeSessions = append(activeSessions, session)
						} else {
							settledSessions = append(settledSessions, session)
						}
					}
					slices.SortStableFunc(settledSessions, func(a, b model.Session) int {
						if a.Pinned != b.Pinned {
							if a.Pinned {
								return -1
							}
							return 1
						}
						return 0
					})
					ui.Row(c).Height(24).Padding(0, 6).Children(func() {
						ui.Text(c, trf("需要处理 · ACTIVE (%d)", len(activeSessions))).FontSize(p.font(fontMicro)).FontWeight(600).TextColor(p.Text3)
						ui.Spacer(c)
						liveColor := p.Text3
						if len(activeSessions) > 0 {
							liveColor = p.AccentText
						}
						ui.Text(c, "LIVE").Font(monoFont).FontSize(p.font(fontMicro)).FontWeight(600).TextColor(liveColor)
					})
					for _, session := range activeSessions {
						d.sessionRow(c, p, session, session.Pinned)
					}
					ui.Row(c).Height(26).Padding(0, 2, 0, 6).Margin(8, 0, 0, 0).Children(func() {
						ui.Text(c, trf("已归档就绪 · SETTLED (%d)", len(settledSessions))).FontSize(p.font(fontMicro)).FontWeight(600).TextColor(p.Text3)
						ui.Spacer(c)
						if iconButton(c, p, "FolderPlus", tr("打开工作区")).Size(22, 22).Clicked() {
							d.chooseWorkspace()
						}
					})
					for _, session := range settledSessions {
						d.sessionRow(c, p, session, session.Pinned)
					}
					if len(d.sessions) == 0 && len(d.recent) == 0 && v.Workspace == "" {
						open := ui.ButtonBase(c).Label(tr("打开本地项目")).FillWidth().Height(72).Margin(6, 0).Radius(10).Border(1, p.BorderStrong).BorderStyle(ui.BorderDashed).Center().Gap(6).Column()
						if open.Hovered() {
							open.Background(p.Hover)
						}
						open.Children(func() {
							icon(c, "FolderOpen", 18, p.Text3)
							ui.Text(c, tr("打开本地项目")).FontSize(p.font(fontBody)).TextColor(p.Text2)
						})
						if open.Clicked() {
							d.chooseWorkspace()
						}
					}
					return
				}
				groups := groupSessions(d.sessions, v.Workspace, d.recent, d.search)
				if len(groups.Pinned) > 0 {
					sideHeading(c, p, tr("置顶"))
					for _, session := range groups.Pinned {
						d.sessionRow(c, p, session, true)
					}
				}
				ui.Row(c).Height(30).Padding(0, 2, 0, 8).Margin(8, 0, 0, 0).Children(func() {
					ui.Text(c, tr("工作空间")).FontSize(p.font(fontCaption)).FontWeight(550).TextColor(p.Text4)
					ui.Spacer(c)
					if iconButton(c, p, "FolderPlus", tr("打开工作区")).Size(24, 24).Clicked() {
						d.chooseWorkspace()
					}
				})
				for _, root := range groups.Roots {
					ui.Column(c.Key("project-" + root)).Children(func() {
						row := ui.ButtonBase(c).Label(tr("工作区 ")+root).Height(32).Radius(7).Gap(8).Padding(0, 4, 0, 8).Justify(ui.Start).Transition(fastMotion)
						if row.Hovered() {
							row.Background(p.Hover)
						}
						row.Children(func() {
							folder := "Folder"
							if !d.collapsedProjects[root] {
								folder = "FolderOpen"
							}
							icon(c, folder, 15, p.Text3)
							ui.Text(c, basename(root)).SingleLine().Shrink(1).FontSize(p.font(fontBody)).FontWeight(550).TextColor(p.Text2)
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
									muted(c, p, tr("暂无任务")).Padding(4, 31).FontSize(p.font(fontCaption)).TextColor(p.Text4)
								}
							})
						}
					})
				}
				if len(groups.Roots) == 0 {
					open := ui.ButtonBase(c).Label(tr("打开本地项目")).FillWidth().Height(76).Margin(6, 0).Radius(10).Border(1, p.BorderStrong).BorderStyle(ui.BorderDashed).Center().Gap(6).Column()
					if open.Hovered() {
						open.Background(p.Hover)
					}
					open.Children(func() {
						icon(c, "FolderOpen", 18, p.Text3)
						ui.Text(c, tr("打开本地项目")).FontSize(p.font(fontBody)).TextColor(p.Text2)
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
	ui.Text(c, label).FontSize(p.font(fontCaption)).FontWeight(550).TextColor(p.Text4).Padding(8, 8, 4, 8)
}

// sidebarFooter is the account of the sidebar: who is working, on how many
// services, with the appearance and the settings at hand.
func (d *desktop) sidebarFooter(c *ui.Context, p palette) {
	footerBg := p.Subtle
	if p.Dark {
		footerBg = p.Subtle.Alpha(0.85)
	}
	ui.Row(c).Height(50).Shrink(0).Padding(0, 8, 0, 10).Gap(9).Background(footerBg).BorderWidth(1, 0, 0, 0).BorderColor(p.BorderSubtle).Children(func() {
		name := userName()
		ui.Box(c).Size(28, 28).Radius(7).Background(p.Ink).Center().Shrink(0).Children(func() {
			ui.Text(c, strings.ToUpper(string([]rune(name)[:1]))).Font(monoFont).FontSize(p.font(fontSmall)).FontWeight(700).TextColor(p.InkText)
		})
		ui.Column(c).Grow(1).Basis(0).MinWidth(0).Children(func() {
			ui.Text(c, name).SingleLine().FontSize(p.font(fontSmall)).FontWeight(600).TextColor(p.Text)
			services := 0
			for _, service := range d.config.Services {
				if service.Enabled {
					services++
				}
			}
			ui.Text(c, trf("本地 · %d 个 AI 服务", services)).SingleLine().Font(monoFont).FontSize(p.font(fontMicro)).TextColor(p.Text3)
		})
		if iconButton(c, p, "Palette", tr("切换主题")).Size(28, 28).Clicked() {
			d.toggleTheme()
		}
		if iconButton(c, p, "Settings2", tr("设置")).Size(28, 28).Clicked() {
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
	ui.Column(c.Key("session-"+session.ID)).Margin(2, 0).Children(func() {
		if d.renameID == session.ID {
			input := ui.TextInput(c, &d.renameText).Label(tr("重命名任务")).Height(32).Radius(7).AutoFocus()
			if input.Submitted() {
				d.sessionAction("renameSession", session.ID, map[string]any{"title": d.renameText})
				d.renameID = ""
			}
			if input.Shortcut(0, ui.KeyEscape) {
				d.renameID = ""
			}
			return
		}
		active, busy, waiting, unread, _ := d.sessionStatus(session.ID)
		busy = busy || session.Status == "running"
		branch, add, del := d.sessionMeta(session)
		row := ui.Row(c).MinHeight(50).Padding(2, 4).Radius(10).Border(1, ui.Transparent).Transition(fastMotion)
		if active {
			row.Background(p.Selected).BorderColor(p.Accent.Alpha(0.28))
			row.DrawOver(func(painter *ui.Painter, r ui.Rect) {
				if r.H > 14 {
					painter.Fill(ui.Rect{X: r.X + 1, Y: r.Y + 7, W: 3, H: r.H - 14}, p.Accent, 2)
				}
			})
			b := row.Bounds()
			if b.H > 0 && d.sessionListBounds.H > 0 {
				d.activeRowY = b.Y - d.sessionListBounds.Y
				d.activeRowSeen = true
			}
		} else if row.Hovered() {
			row.Background(p.Hover)
		}
		row.Children(func() {
			main := ui.ButtonBase(c).Label(session.Title).Grow(1).Basis(0).MinWidth(0).Column().AlignItems(ui.Stretch).Gap(4).Padding(5, 6, 5, 8)
			main.Children(func() {
				ui.Row(c).Gap(7).Children(func() {
					switch {
					case waiting:
						pulse(c, 7, 7, 3.5, p.Warning, 1600*time.Millisecond, 0.5, 1).Tooltip(tr("等待你的确认"))
					case busy:
						pulse(c, 7, 7, 3.5, p.Accent, 1600*time.Millisecond, 0.45, 1)
					case unread:
						ui.Box(c).Size(7, 7).Radius(99).Background(p.Accent).Shrink(0)
					default:
						ui.Box(c).Size(7, 7).Radius(99).Background(p.Success.Alpha(0.85)).Shrink(0)
					}
					if pinned {
						icon(c, "Pin", 11, p.Text4)
					}
					color, weight := p.Text2, 450
					if active {
						color, weight = p.Text, 600
					}
					ui.Text(c, session.Title).Grow(1).Basis(0).MinWidth(0).SingleLine().FontSize(p.font(fontBody)).FontWeight(weight).TextColor(color)
					if !row.Hovered() {
						stamp := session.UpdatedAt
						if stamp == "" {
							stamp = session.CreatedAt
						}
						ui.Text(c, relativeTime(stamp)).NoWrap().Shrink(0).Font(monoFont).FontSize(p.font(fontMicro)).TextColor(p.Text3)
					}
				})
				ui.Row(c).Padding(0, 0, 0, 14).Gap(5).Children(func() {
					if branch != "" {
						ui.Row(c).Height(17).Padding(0, 5).Gap(4).Radius(4).Background(p.Subtle).Border(1, p.BorderSubtle).Shrink(0).Children(func() {
							icon(c, "GitBranch", 9, p.Text3)
							ui.Text(c, branch).NoWrap().Font(monoFont).FontSize(p.font(fontMicro)).TextColor(p.Text2)
						})
					}
					if session.Workspace != "" {
						ui.Text(c, basename(session.Workspace)).SingleLine().Shrink(1).MinWidth(0).Font(monoFont).FontSize(p.font(fontMicro)).TextColor(p.Text3)
					}
					if add+del > 0 {
						ui.Spacer(c)
						d.diffStat(c, p, add, del)
					}
				})
			})
			if main.Clicked() {
				d.openSession(session)
			}
			if main.DoubleClicked() {
				d.renameID = session.ID
				d.renameText = session.Title
			}
			if row.Hovered() {
				iconButton(c, p, "Ellipsis", tr("任务操作")).Size(24, 24).Menu(func(menu *ui.Menu) { d.sessionMenu(menu, session) })
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
		if d.preferences.RightCollapsed {
			ui.Row(c).Absolute().Top(10).Right(10).Children(func() {
				if iconButton(c, p, "PanelRightOpen", tr("展开工作区")).Size(30, 30).Clicked() {
					d.preferences.RightCollapsed = false
					d.persist()
				}
			})
		}
		ui.Scroll(c).Fill().Children(func() {
			ui.Column(c).FillWidth().MinHeight(height - 64).Center().Children(func() {
				ui.Column(c).FillWidth().MaxWidth(720).Padding(32, 32, 48, 32).Margin(0, ui.Auto).Children(func() {
					ui.Row(c).FillWidth().Justify(ui.Center).Gap(14).Margin(0, 0, 26, 0).Transition(enterMotion).Children(func() {
						spark(c, 34, p.Accent, false)
						ui.Text(c, greeting(time.Now())+tr("，")+userName()).Font(p.Prose).FontSize(p.font(fontHero)).FontWeight(600).TextColor(p.Text).LetterSpacing(-0.4).SingleLine().MinWidth(0).Shrink(1)
					})
					d.composer(c, p, true)
					ui.Row(c).Wrap().Justify(ui.Center).Gap(8).Margin(16, 0, 0, 0).Children(func() {
						for _, s := range suggestionPrompts {
							pill := ui.ButtonBase(c).Label(tr(s.Label)).Height(34).Padding(0, 14, 0, 12).Gap(7).Radius(7).Border(1, p.Border).Background(p.Elevated.Alpha(0.55)).Transition(fastMotion)
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
								ui.Text(c, tr(s.Label)).NoWrap().FontSize(p.font(fontBody)).TextColor(p.Text2)
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
			ui.Text(c, tr("最近任务")).FontSize(p.font(fontSmall)).FontWeight(550).TextColor(p.Text3)
		})
		ui.Column(c).Gap(1).Children(func() {
			for _, session := range recent {
				row := ui.ButtonBase(c.Key("recent-"+session.ID)).Label(tr("打开任务 ")+session.Title).FillWidth().Height(38).Padding(0, 10).Gap(10).Radius(7).Justify(ui.Start).Transition(fastMotion)
				if row.Hovered() {
					row.Background(p.Hover)
				}
				row.Children(func() {
					icon(c, "MessageSquare", 14, p.Text3)
					ui.Text(c, session.Title).Grow(1).Basis(0).MinWidth(0).SingleLine().FontSize(p.font(fontBody)).TextColor(p.Text2)
					ui.Text(c, basename(session.Workspace)).NoWrap().Shrink(0).Font(monoFont).FontSize(p.font(fontCaption)).TextColor(p.Text4)
					stamp := session.UpdatedAt
					if stamp == "" {
						stamp = session.CreatedAt
					}
					ui.Text(c, relativeTime(stamp)).NoWrap().Shrink(0).Width(52).TextAlign(ui.End).Font(monoFont).FontSize(p.font(fontCaption)).TextColor(p.Text4)
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
	if c.Shortcut(ui.Cmd, ui.KeyT) {
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
		if c.Shortcut(ui.Alt, key) {
			tabs := d.titlebarTabs()
			if i < len(tabs) {
				if tabs[i].Session.ID != "" {
					d.openSession(tabs[i].Session)
				} else if tabs[i].DraftKey != "" {
					d.activate(tabs[i].DraftKey)
				}
			}
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
	order := []string{"mono-carbon", "t3-noir", "warm-ink", "daylight-paper"}
	current := resolveThemePreset(d.preferences.Theme, d.preferences.DarkPreset, d.lastDark)
	next := order[0]
	for i, id := range order {
		if id == current {
			next = order[(i+1)%len(order)]
			break
		}
	}
	d.preferences.Theme = next
	if next != "daylight-paper" {
		d.preferences.DarkPreset = next
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
				glassy(toastCard, 10, p.Elevated.Alpha(0.78)).Children(func() {
					ui.Row(c).Gap(8).Children(func() {
						icon(c, "CircleCheck", 15, p.Success)
						ui.Text(c, tr(entry.Title)).FontSize(p.font(fontBody)).FontWeight(600).Grow(1)
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
