package main

import (
	"encoding/json"
	"fmt"
	"math"
	"strings"
	"time"

	"github.com/egoist/mygo/ui"
	"minicode.desktop/internal/content"
	"minicode.desktop/internal/model"
)

var toolNames = map[string]struct{ Label, Done, Icon string }{
	"read": {"读取", "已读取", "FileText"}, "ls": {"浏览", "已浏览", "FolderOpen"},
	"grep": {"搜索", "已搜索", "Search"}, "edit": {"编辑", "已编辑", "PencilLine"},
	"write": {"写入", "已写入", "FilePlus2"}, "bash": {"运行", "已运行", "Terminal"}, "powershell": {"运行", "已运行", "Terminal"},
	"delegate": {"子助手", "已委派", "Users"}, "read_artifact": {"读取归档", "已读取归档", "Archive"},
}

func toolTarget(item *model.FeedItem) string {
	for _, key := range []string{"path", "command", "pattern", "task", "artifact_id"} {
		if value := model.String(item.Args[key]); value != "" {
			return value
		}
	}
	return item.Name
}

// feedWidth is the measure of the conversation, as Claude keeps its column.
const feedWidth = 800

func (d *desktop) feed(c *ui.Context, p palette) {
	v := d.current()
	ui.Column(c).Fill().Children(func() {
		if v.Loading {
			ui.Row(c).Grow(1).Center().Gap(8).Children(func() { spinner(c, p); muted(c, p, tr("正在恢复会话…")) })
			return
		}
		if v.History != nil && v.History.Cursor.HasMore() {
			if button(c, p, tr("加载更早历史"), "ChevronUp", false).Disabled(v.HistoryLoading).Clicked() {
				d.loadEarlierHistory(v)
			}
		}
		v.List.FollowEnd = true
		if !reduceMotion(c) {
			v.List.FollowAnimation = &ui.Smooth
		} else {
			v.List.FollowAnimation = nil
		}
		v.List.Key = func(i int) any { return v.Rows[i].ID }
		bottom := max(150, d.dockHeight) + 12
		ui.List(c, &v.List, len(v.Rows), func(i int) {
			row := v.Rows[i]
			ui.Column(c).FillWidth().MaxWidth(feedWidth).Margin(0, ui.Auto).Padding(0, 32).Children(func() {
				if row.Tools != nil {
					if len(row.Tools) > 1 {
						d.toolGroup(c, p, row)
					} else {
						d.toolGroup(c, p, row)
					}
				} else {
					d.message(c, p, row.Item, i == len(v.Rows)-1)
				}
			})
		}).Grow(1).Gap(18).Padding(8, 0, bottom, 0).ScrollbarInsets(8, 0, bottom-12, 0).Label(tr("对话记录")).Role(ui.RoleList)
		if !v.List.AtEnd() {
			ui.Row(c).Absolute().Bottom(bottom - 4).Left(0).Right(0).Justify(ui.Center).PassThrough().Children(func() {
				back := glassy(ui.ButtonBase(c).Label(tr("回到最新")).Height(32).Padding(0, 14, 0, 12).Gap(6), 99, ui.Transparent).Transition(enterMotion)
				back.Children(func() {
					icon(c, "ArrowDown", 14, p.Text2)
					ui.Text(c, tr("回到最新")).NoWrap().FontSize(p.font(12.04)).FontWeight(550).TextColor(p.Text2)
				})
				if back.Clicked() {
					v.List.ScrollToEnd()
				}
			})
		}
	})
}

func (d *desktop) message(c *ui.Context, p palette, item *model.FeedItem, last bool) {
	v := d.current()
	switch item.Kind {
	case "user":
		turn := ui.Column(c).AlignItems(ui.End).Gap(6).Margin(10, 0, 0, 0).Transition(enterMotion)
		turn.Children(func() {
			if len(item.Attachments) > 0 {
				d.attachmentChips(c, p, item.Attachments, false)
			}
			ui.Text(c, item.Text).MaxWidthPercent(82).Padding(10, 16).Radius(20).Background(p.Subtle).FontSize(p.font(14)).LineHeight(1.6).TextColor(p.Text).Selectable()
			actions := ui.Row(c).Height(26).Gap(2).Margin(-2, 0, 0, 0)
			if !turn.Hovered() && !turn.FocusWithin() {
				actions.Opacity(0)
			}
			actions.Transition(fastMotion).Children(func() {
				if iconButton(c, p, "Copy", tr("复制消息")).Size(26, 26).Clicked() {
					c.WriteClipboard(item.Text)
				}
				if !v.Busy && iconButton(c, p, "PencilLine", tr("编辑并重新发送")).Size(26, 26).Clicked() {
					v.Draft = item.Text
					v.Attachments = append([]model.Attachment(nil), item.Attachments...)
					d.focusPrompt = true
				}
			})
		})
	case "assistant":
		turn := ui.Column(c).Gap(6).Transition(enterMotion)
		turn.Children(func() {
			d.markdown(c, p, item.Text)
			if item.Pending {
				pulse(c, 8, 16, 2, p.Text3, time.Second, 0.15, 1)
			} else {
				actions := ui.Row(c).Height(28).Gap(2).Margin(2, 0, 0, -4)
				if !last && !turn.Hovered() && !turn.FocusWithin() {
					actions.Opacity(0)
				}
				actions.Transition(fastMotion).Children(func() {
					if last {
						ui.Box(c).Size(28, 28).Center().Children(func() { spark(c, 16, p.Accent, false) })
					}
					if iconButton(c, p, "Copy", tr("复制回复")).Size(28, 28).Clicked() {
						c.WriteClipboard(item.Text)
					}
					if !v.Busy && last && iconButton(c, p, "RotateCcw", tr("重新生成")).Size(28, 28).Clicked() {
						d.promptRetry()
					}
				})
			}
		})
	case "thinking":
		ui.Row(c).Height(28).Gap(10).Transition(enterMotion).Children(func() {
			spark(c, 18, p.Accent, true)
			shimmer(c, p, tr(item.Text)).Grow(1).Basis(0).MinWidth(0)
			if elapsed := time.Since(item.StartedAt); elapsed >= 2*time.Second {
				muted(c, p, formatDuration(elapsed)).NoWrap().Shrink(0).FontSize(p.font(11.2)).TextColor(p.Text4)
			}
		})
		c.After(time.Second)
	case "reasoning":
		d.reasoningView(c, p, item)
	case "notice":
		color := p.Text3
		name := "Info"
		if item.Tone == "warning" {
			color = p.Warning
			name = "TriangleAlert"
		}
		if item.Tone == "error" {
			color = p.Danger
			name = "CircleAlert"
		}
		ui.Row(c).Gap(8).Padding(4, 0).AlignItems(ui.Start).Transition(enterMotion).Children(func() {
			ui.Box(c).Height(20).Center().Children(func() { icon(c, name, 14, color) })
			ui.Text(c, localizeNotice(item.Text)).Grow(1).FontSize(p.font(12.32)).TextColor(p.Text2).Selectable()
		})
	case "summary":
		label, name, color := tr("已完成"), "CircleCheck", p.Text4
		if item.Interrupted {
			label, name, color = tr("已停止"), "CircleSlash", p.Warning
		} else if !item.Success {
			label, name, color = tr("已结束"), "TriangleAlert", p.Warning
		}
		text := label + tr(" · 用时 ") + formatDuration(item.EndedAt.Sub(item.StartedAt))
		if item.Text != "" && item.Text != "0" {
			text += " · " + item.Text + tr(" 个操作")
		}
		ui.Row(c).Height(24).Gap(10).Children(func() {
			ui.Box(c).Grow(1).Height(1).Background(p.BorderSubtle)
			icon(c, name, 12, color)
			ui.Text(c, text).NoWrap().Shrink(0).FontSize(p.font(11.2)).TextColor(p.Text4)
			ui.Box(c).Grow(1).Height(1).Background(p.BorderSubtle)
		})
	case "approval":
		d.approval(c, p, item)
	}
}

// shimmer sets text with a soft band of light moving across it, the way
// Claude shows a step in progress, in frames that only paint.
func shimmer(c *ui.Context, p palette, text string) ui.Element {
	still := reduceMotion(c)
	e := ui.Text(c, text).SingleLine().FontSize(p.font(13)).TextColor(p.Text2)
	e.DrawOver(func(painter *ui.Painter, r ui.Rect) {
		if still || r.W <= 0 {
			return
		}
		painter.AnimationFrame()
		period := int64(2200)
		t := float32(painter.Now().UnixMilli()%period) / float32(period)
		band := max(48, r.W*0.35)
		x := r.X - band + (r.W+2*band)*t
		veil := p.Main.Alpha(0.62)
		painter.Clip(r, 0, func() {
			painter.Fill(ui.Rect{X: r.X, Y: r.Y, W: max(0, x-r.X), H: r.H}, veil, 0)
			painter.FillGradient(ui.Rect{X: x, Y: r.Y, W: band / 2, H: r.H}, ui.LinearGradient{From: veil, To: ui.Transparent, Angle: 90}, 0)
			painter.FillGradient(ui.Rect{X: x + band/2, Y: r.Y, W: band / 2, H: r.H}, ui.LinearGradient{From: ui.Transparent, To: veil, Angle: 90}, 0)
			painter.Fill(ui.Rect{X: x + band, Y: r.Y, W: max(0, r.X+r.W-x-band), H: r.H}, veil, 0)
		})
	})
	return e
}

// disclosure is a chevron that turns down as what it discloses opens.
func disclosure(c *ui.Context, p palette, open bool) {
	chevron := ui.Box(c).Size(14, 14).Center().Shrink(0).Children(func() { icon(c, "ChevronRight", 12, p.Text4) })
	turn := float32(0)
	if open {
		turn = 90
	}
	chevron.AnimatePose(ui.Pose{Rotate: turn}, ui.Snappy)
}

func (d *desktop) reasoningView(c *ui.Context, p palette, item *model.FeedItem) {
	v := d.current()
	key := "reasoning-" + item.ID
	open, stored := v.Open[key]
	if !stored {
		open = item.Pending
	}
	root := ui.Column(c.Key(key)).FillWidth().Gap(6).Transition(enterMotion)
	scroll := ui.Local(root, "scroll", func() ui.ScrollState { return ui.ScrollState{} })
	root.Children(func() {
		label := tr("思考过程")
		if !item.Pending && !item.StartedAt.IsZero() && !item.EndedAt.IsZero() {
			label = trf("思考了 %s", formatDuration(item.EndedAt.Sub(item.StartedAt)))
		} else if item.Interrupted {
			label = tr("思考已中断")
		}
		head := ui.ButtonBase(c).Label(tr("思考过程")).Description(tr("模型接口返回的推理内容")).Expanded(open).Height(28).Padding(0, 8, 0, 6).Gap(6).Radius(9).Justify(ui.Start).Transition(fastMotion)
		if head.Hovered() {
			head.Background(p.Hover)
		}
		head.Children(func() {
			if item.Pending {
				spark(c, 14, p.Accent, true)
				shimmer(c, p, tr("正在思考…")).FontSize(p.font(12.6))
			} else {
				icon(c, "Clock", 14, p.Text3)
				ui.Text(c, label).NoWrap().FontSize(p.font(12.6)).TextColor(p.Text3)
			}
			disclosure(c, p, open)
		})
		if head.Clicked() {
			v.Open[key] = !open
		}
		if open {
			if item.Pending && scroll.Y >= scroll.MaxY-1 {
				scroll.Y = math.MaxFloat32
			}
			ui.Scroll(c).FillWidth().MaxHeight(220).TrackScroll(scroll).Margin(0, 0, 4, 13).Padding(2, 12, 2, 14).BorderWidth(0, 0, 0, 2).BorderColor(p.Border).Transition(enterMotion).Children(func() {
				ui.Text(c, item.Text).FillWidth().Selectable().FontSize(p.font(12.6)).LineHeight(1.65).TextColor(p.Text3)
			})
		}
	})
}

// toolMeta is the short result a step shows at its end: lines read,
// matches found, lines changed, the exit of a command.
func (d *desktop) toolMeta(c *ui.Context, p palette, item *model.FeedItem) {
	if item.Pending {
		return
	}
	lines := func(s string) int {
		s = strings.TrimRight(s, "\n")
		if s == "" {
			return 0
		}
		return strings.Count(s, "\n") + 1
	}
	small := func(text string, color ui.Color) {
		ui.Text(c, text).NoWrap().Shrink(0).FontSize(p.font(11.2)).TextColor(color)
	}
	if !item.Success && !item.Interrupted {
		small(tr("失败"), p.Danger)
		return
	}
	switch item.Name {
	case "read":
		if n := lines(item.Output); n > 0 {
			small(trf("%d 行", n), p.Text4)
		}
	case "grep":
		if n := lines(item.Output); n > 0 {
			small(trf("%d 处匹配", n), p.Text4)
		}
	case "edit", "write":
		add, del := 0, 0
		for _, line := range d.itemDiff(item) {
			switch line.Type {
			case "add":
				add++
			case "remove":
				del++
			}
		}
		if add+del > 0 {
			d.diffStat(c, p, add, del)
		}
	case "bash", "powershell":
		if item.Success {
			ui.Row(c).Gap(4).Shrink(0).Children(func() {
				icon(c, "Check", 12, p.Success)
				small(tr("完成"), p.Success)
			})
		}
	}
	if !item.StartedAt.IsZero() && !item.EndedAt.IsZero() {
		small(fmt.Sprintf("%.1fs", item.EndedAt.Sub(item.StartedAt).Seconds()), p.Text4)
	}
}

// toolGroup is the activity of a turn: what the agent did, in a card that
// sums it up, and opens on a timeline of each step.
func (d *desktop) toolGroup(c *ui.Context, p palette, row model.FeedRow) {
	v := d.current()
	running := false
	counts := map[string]int{}
	failed := 0
	var elapsed time.Duration
	for _, item := range row.Tools {
		if item.Pending {
			running = true
		}
		if item.Name == "powershell" {
			counts["bash"]++
		} else {
			counts[item.Name]++
		}
		if !item.Success && !item.Pending && !item.Interrupted {
			failed++
		}
		if !item.StartedAt.IsZero() && !item.EndedAt.IsZero() {
			elapsed += item.EndedAt.Sub(item.StartedAt)
		}
	}
	single := len(row.Tools) == 1
	open, stored := v.Open["group-"+row.ID]
	if !stored {
		open = running || d.preferences.ExpandTools || single
	}
	if single {
		d.tool(c, p, row.Tools[0], false)
		return
	}
	var names []string
	for _, entry := range []struct{ Name, Format string }{{"read", tr("读取 %d")}, {"grep", tr("搜索 %d")}, {"edit", tr("编辑 %d")}, {"write", tr("写入 %d")}, {"bash", tr("命令 %d")}, {"delegate", tr("委派 %d")}} {
		if count := counts[entry.Name]; count > 0 {
			names = append(names, fmt.Sprintf(entry.Format, count))
		}
	}
	if failed > 0 {
		names = append(names, fmt.Sprintf(tr("%d 个失败"), failed))
	}
	if elapsed > 0 && !running {
		names = append(names, fmt.Sprintf(tr("%.1f 秒"), elapsed.Seconds()))
	}
	group := ui.Column(c).Border(1, p.BorderSubtle).Radius(16).Background(p.Elevated.Alpha(0.6)).Clip().Transition(fastMotion)
	if open {
		group.BorderColor(p.Border)
	}
	group.Children(func() {
		label := fmt.Sprintf(tr("已执行 %d 个操作"), len(row.Tools))
		if running {
			label = fmt.Sprintf(tr("正在执行 %d 个操作"), len(row.Tools))
		}
		head := ui.ButtonBase(c).Label(label).Expanded(open).FillWidth().Height(46).Gap(10).Padding(0, 14).Transition(fastMotion)
		if head.Hovered() {
			head.Background(p.Hover)
		}
		head.Children(func() {
			if running {
				ring(c, 15, p.Text3)
			} else {
				icon(c, "Layers", 15, p.Text3)
			}
			ui.Text(c, label).NoWrap().Shrink(0).FontSize(p.font(13)).FontWeight(550).TextColor(p.Text)
			ui.Spacer(c)
			if len(names) > 0 {
				ui.Text(c, strings.Join(names, " · ")).SingleLine().Shrink(1).FontSize(p.font(11.76)).TextColor(p.Text3)
			}
			disclosure(c, p, open)
		})
		if head.Clicked() {
			v.Open["group-"+row.ID] = !open
		}
		if open {
			ui.Column(c).Padding(6, 10, 10, 10).BorderWidth(1, 0, 0, 0).BorderColor(p.BorderSubtle).Transition(ui.ElementTransition{Duration: 240 * time.Millisecond, Ease: ui.CubicBezier(0.2, 0.8, 0.2, 1), Enter: &ui.Motion{Collapse: true}, Exit: &ui.Motion{Collapse: true}}).Children(func() {
				for i, item := range row.Tools {
					d.step(c, p, item, i < len(row.Tools)-1)
				}
			})
		}
		if !open {
			for _, item := range row.Tools {
				if item.Success && len(item.Attachments) > 0 {
					ui.Column(c).Padding(0, 14, 14).Children(func() { d.readImages(c, p, item.Attachments) })
				}
			}
		}
	})
}

// tool shows one step on its own, as a group of one.
func (d *desktop) tool(c *ui.Context, p palette, item *model.FeedItem, force bool) {
	ui.Column(c).Border(1, p.BorderSubtle).Radius(16).Background(p.Elevated.Alpha(0.6)).Padding(4, 10).Clip().Children(func() {
		d.stepWith(c, p, item, false, force)
	})
}

func (d *desktop) step(c *ui.Context, p palette, item *model.FeedItem, more bool) {
	d.stepWith(c, p, item, more, false)
}

// stepWith is a step of the timeline: its icon on the line that joins the
// steps, what it did and to what, its result, and what it showed, opened.
func (d *desktop) stepWith(c *ui.Context, p palette, item *model.FeedItem, more, force bool) {
	v := d.current()
	meta := toolNames[item.Name]
	meta.Label, meta.Done = tr(meta.Label), tr(meta.Done)
	if meta.Label == "" {
		meta.Label = item.Name
		meta.Done = item.Name
		meta.Icon = "Wrench"
	}
	waiting := false
	for _, approval := range v.Items {
		if approval.ApprovalID != "" && approval.Name == item.Name && item.Pending {
			waiting = true
		}
	}
	open, stored := v.Open["tool-"+item.ID]
	if !stored {
		open = force || d.preferences.ExpandTools || (item.Pending && (item.Name == "bash" || item.Name == "powershell") && !waiting) || ((item.Name == "edit" || item.Name == "write") && item.Success)
	}
	ui.Row(c).AlignItems(ui.Stretch).Gap(10).Children(func() {
		ui.Column(c).Width(20).AlignItems(ui.Center).Shrink(0).Children(func() {
			ui.Box(c).Height(34).Center().Children(func() {
				switch {
				case item.Pending && !waiting:
					ring(c, 14, p.Text3)
				case !item.Success && !item.Pending && !item.Interrupted:
					icon(c, meta.Icon, 15, p.Danger)
				default:
					icon(c, meta.Icon, 15, p.Text3)
				}
			})
			if more {
				ui.Box(c).Width(1).Grow(1).Background(p.BorderSubtle)
			}
		})
		ui.Column(c).Grow(1).Basis(0).MinWidth(0).Children(func() {
			head := ui.ButtonBase(c).Label(meta.Label+" "+toolTarget(item)).Expanded(open).FillWidth().Height(34).Gap(8).Padding(0, 6).Radius(9).Transition(fastMotion)
			if head.Hovered() {
				head.Background(p.Hover)
			}
			head.Children(func() {
				label := meta.Label
				if !item.Pending {
					label = strings.TrimPrefix(meta.Done, "已")
				}
				ui.Text(c, label).NoWrap().Shrink(0).FontSize(p.font(12.6)).TextColor(p.Text)
				target := toolTarget(item)
				if item.Name == "grep" {
					target = fmt.Sprintf("\"%s\"", model.String(item.Args["pattern"]))
					if path := model.String(item.Args["path"]); path != "" {
						target += tr(" 于 ") + path
					}
				}
				ui.Text(c, target).Grow(1).Basis(0).MinWidth(0).SingleLine().Font(monoFont).FontSize(p.font(11.48)).TextColor(p.Text3)
				switch {
				case waiting:
					badge(c, p, tr("等待批准"), p.Warning, p.WarningSoft)
				case item.Interrupted:
					badge(c, p, tr("已中断"), p.Warning, p.WarningSoft)
				case item.Auto:
					badge(c, p, tr("自动批准"), p.Text3, p.Hover)
				}
				d.toolMeta(c, p, item)
			})
			if head.Clicked() {
				v.Open["tool-"+item.ID] = !open
			}
			if item.Success && len(item.Attachments) > 0 {
				ui.Column(c).Margin(2, 0, 8, 6).Children(func() { d.readImages(c, p, item.Attachments) })
			}
			if open {
				ui.Column(c).Margin(0, 0, 8, 6).Gap(8).Transition(ui.ElementTransition{Duration: 220 * time.Millisecond, Ease: ui.CubicBezier(0.2, 0.8, 0.2, 1), Enter: &ui.Motion{Collapse: true}, Exit: &ui.Motion{Collapse: true}}).Children(func() {
					d.toolDetail(c, p, v, item)
				})
			}
		})
	})
}

func (d *desktop) toolDetail(c *ui.Context, p palette, v *conversation, item *model.FeedItem) {
	switch item.Name {
	case "bash", "powershell":
		d.terminal(c, p, model.String(item.Args["command"]), item.Output)
	case "edit", "write":
		if item.Success || item.Pending {
			d.diffView(c, p, d.itemDiff(item), item.ID, 40)
		} else if item.Output != "" {
			logText(c, p, item.Output, 180)
		}
	default:
		if item.Output != "" {
			logText(c, p, item.Output, 280)
		} else {
			data, _ := json.MarshalIndent(item.Args, "", "  ")
			logText(c, p, string(data), 220)
		}
	}
	if item.ChildSessionID != "" && button(c, p, tr("查看子助手会话"), "ArrowUpRight", false).Clicked() {
		for _, session := range d.sessions {
			if session.ID == item.ChildSessionID {
				d.openSession(session)
				return
			}
		}
		var detail model.Detail
		d.request(v, "getSession", map[string]any{"sessionId": item.ChildSessionID}, &detail, func(err error) {
			if err != nil {
				d.error(v, err)
			} else {
				d.openSession(detail.Summary)
			}
		})
	}
	if item.ArtifactID != "" {
		a := v.Artifacts[item.ID]
		if a == nil && button(c, p, tr("查看完整归档输出"), "Archive", false).Clicked() {
			d.loadArtifact(item)
		}
		if a != nil {
			if a.Text != "" {
				logText(c, p, a.Text, 320)
			}
			if a.Error != "" {
				ui.Text(c, a.Error).TextColor(p.Danger)
			}
			if a.HasMore && button(c, p, tr("加载更多"), "ChevronDown", false).Disabled(a.Loading).Clicked() {
				d.loadArtifact(item)
			}
			if a.Loading {
				spinner(c, p)
			}
		}
	}
}

func (d *desktop) terminal(c *ui.Context, p palette, command, output string) {
	ui.Column(c).Padding(10, 14).Radius(12).Background(p.Terminal).Border(1, p.CodeBorder).Gap(6).Children(func() {
		if command != "" {
			ui.RichText(c, ui.Span{Text: "$ ", Color: p.TerminalDim}, ui.Span{Text: command, Color: p.TerminalText}).Font(monoFont).FontSize(p.font(12.04)).Selectable()
		}
		if output != "" {
			text := output
			ui.TextAreaBase(c, &text).ReadOnly(true).Label(tr("命令输出")).Height(min(280, float32(strings.Count(output, "\n")+1)*19+8)).Font(monoFont).FontSize(p.font(11.76)).TextColor(p.TerminalDim).LineHeight(1.6)
		}
	})
}
func logText(c *ui.Context, p palette, source string, maxHeight float32) {
	text := source
	ui.Column(c).Radius(12).Background(p.Code).Border(1, p.CodeBorder).Padding(8, 12).Children(func() {
		ui.TextAreaBase(c, &text).ReadOnly(true).Label(tr("工具输出")).Height(min(maxHeight, float32(strings.Count(source, "\n")+1)*18+8)).Font(monoFont).FontSize(p.font(11.48)).TextColor(p.Text2).LineHeight(1.6)
	})
}

func (d *desktop) approval(c *ui.Context, p palette, item *model.FeedItem) {
	if item.ApprovalID == "" {
		label := tr("已拒绝")
		name, color := "ShieldX", p.Text4
		if item.Granted {
			label = tr("已批准")
			name, color = "ShieldCheck", p.Text3
		}
		ui.Row(c).Gap(8).Height(26).Children(func() {
			icon(c, name, 14, color)
			ui.Text(c, label+" · "+toolTarget(item)).SingleLine().FontSize(p.font(12.04)).TextColor(p.Text3)
		})
		return
	}
	card := ui.Column(c).Radius(18).Clip().Border(1, p.Border).Background(p.Elevated).Transition(ui.ElementTransition{Duration: 260 * time.Millisecond, Ease: ui.CubicBezier(0.2, 0.8, 0.2, 1), Enter: &ui.Motion{Y: 10, Scale: 0.98}}).Role(ui.RoleGroup).Label(tr("需要你的批准"))
	card.Shadow(0, 10, 30, -10, ui.RGBA(0, 0, 0, 0.16))
	question, iconName := tr("允许 MiniCode 编辑这个文件吗？"), "PencilLine"
	switch item.Name {
	case "bash", "powershell":
		question, iconName = tr("允许 MiniCode 运行这条命令吗？"), "Terminal"
	case "write":
		question, iconName = tr("允许 MiniCode 写入这个文件吗？"), "FilePlus2"
	case "workspace_trust":
		question, iconName = tr("信任此工作区的插件并启动其 MCP 服务吗？"), "ShieldCheck"
	case "edit":
	default:
		question, iconName = trf("允许 MiniCode 使用 %s 吗？", item.Name), "Wrench"
	}
	card.Children(func() {
		ui.Row(c).Padding(16, 18, 12, 18).Gap(12).Children(func() {
			ui.Box(c).Size(34, 34).Radius(10).Background(p.WarningSoft).Center().Shrink(0).Children(func() { icon(c, iconName, 17, p.Warning) })
			ui.Column(c).Grow(1).Gap(2).Children(func() {
				ui.Text(c, question).FontSize(p.font(14)).FontWeight(600).TextColor(p.Text)
				muted(c, p, tr("需要你的批准")+" · "+toolTarget(item)).FontSize(p.font(11.48)).SingleLine()
			})
		})
		ui.Column(c).Padding(0, 18, 14, 18).Children(func() {
			if item.Name == "bash" || item.Name == "powershell" {
				d.terminal(c, p, model.String(item.Args["command"]), "")
			} else {
				ui.Text(c, model.String(item.Args["path"])).Font(monoFont).FontSize(p.font(11.48)).TextColor(p.Text2).Margin(0, 0, 8, 0)
				if item.Name == "edit" {
					d.diffView(c, p, d.itemDiff(item), item.ID, 40)
				} else {
					text := model.String(item.Args["content"])
					if text == "" {
						data, _ := json.MarshalIndent(item.Args, "", "  ")
						text = string(data)
					}
					logText(c, p, text, 240)
				}
			}
		})
		ui.Row(c).Padding(12, 18, 16, 18).Gap(8).Justify(ui.End).Children(func() {
			key := func(label, k string, primary bool) ui.Element {
				b := styleButton(ui.ButtonBase(c).Label(label), p, primary).Height(34).Radius(11).Padding(0, 12, 0, 14)
				b.Children(func() {
					ui.Text(c, label).NoWrap()
					color, bg := p.Text3, p.Hover
					if primary {
						color, bg = p.InkText.Alpha(0.7), p.InkText.Alpha(0.14)
					}
					ui.Text(c, k).Font(monoFont).FontSize(p.font(10.36)).Padding(1, 5).Radius(5).TextColor(color).Background(bg)
				})
				return b
			}
			if key(tr("拒绝"), "N", false).Disabled(item.Deciding).Clicked() {
				d.decide(item, false, false)
			}
			// Trust is kept per plugin fingerprint; "always allow" has no meaning there.
			trust := item.Name == "workspace_trust"
			if !trust && key(tr("本会话始终允许 ")+item.Name, "A", false).Disabled(item.Deciding).Clicked() {
				d.decide(item, true, true)
			}
			approve := tr("批准执行")
			if trust {
				approve = tr("信任并启动")
			}
			if key(approve, "Y", true).Disabled(item.Deciding).Clicked() {
				d.decide(item, true, false)
			}
		})
	})
	if card.Shortcut(0, ui.KeyY) {
		d.decide(item, true, false)
	}
	if card.Shortcut(0, ui.KeyA) {
		d.decide(item, true, true)
	}
	if card.Shortcut(0, ui.KeyN) || card.Shortcut(0, ui.KeyEscape) {
		d.decide(item, false, false)
	}
}

func (d *desktop) diffView(c *ui.Context, p palette, lines []content.DiffLine, id string, limit int) {
	v := d.current()
	expanded := v.Open["diff-all-"+id]
	if len(lines) == 0 {
		muted(c, p, tr("没有可显示的差异"))
		return
	}
	count := len(lines)
	if limit > 0 && !expanded {
		count = min(limit, count)
	}
	frame := ui.Column(c).Clip().Radius(12).Border(1, p.CodeBorder).Background(p.Code).Padding(4, 0).Font(monoFont).FontSize(p.font(11.76))
	if id == "preview-row" {
		// The preview lists the lines one by one; each is a bare row.
		frame.Radius(0).Border(0, ui.Transparent).Padding(0).Background(ui.Transparent)
	}
	frame.Children(func() {
		for _, line := range lines[:count] {
			bg, color, number, sign := ui.Transparent, p.Text2, p.Text4, " "
			switch line.Type {
			case "add":
				bg, color, number, sign = p.AddBG, p.AddText, p.AddText.Alpha(0.7), "+"
			case "remove":
				bg, color, number, sign = p.RemoveBG, p.RemoveText, p.RemoveText.Alpha(0.7), "−"
			case "hunk":
				color = p.Text4
			case "meta":
				color = p.Text3
			}
			ui.Row(c).Background(bg).AlignItems(ui.Stretch).Children(func() {
				if line.Type != "meta" && line.Type != "hunk" {
					n := line.NewLine
					if line.Type == "remove" || n <= 0 {
						n = line.OldLine
					}
					label := ""
					if n > 0 {
						label = fmtInt(n)
					}
					ui.Text(c, label).Width(44).TextAlign(ui.End).Padding(2, 8, 2, 4).TextColor(number).FontSize(p.font(10.64))
					ui.Text(c, sign).Width(16).TextColor(color).Padding(2, 0)
				}
				text := line.Text
				if text == "" {
					text = " "
				}
				row := ui.Text(c, text).Grow(1).Padding(2, 10, 2, 0).TextColor(color).NoWrap().Selectable().LineHeight(1.6)
				if line.Type == "hunk" {
					row.Padding(2, 14).FontSize(p.font(10.64))
				}
			})
		}
		if count < len(lines) && button(c, p, fmt.Sprintf(tr("显示全部 %d 行"), len(lines)), "ChevronDown", false).FillWidth().Border(0, ui.Transparent).Radius(0).Clicked() {
			v.Open["diff-all-"+id] = true
		}
	})
}

func filepathExt(path string) string {
	index := strings.LastIndex(path, ".")
	if index < 0 {
		return ""
	}
	return path[index:]
}
