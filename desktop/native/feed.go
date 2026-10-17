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
	"write": {"写入", "已写入", "FilePlus2"}, "bash": {"运行", "已运行", "Terminal"},
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

func (d *desktop) feed(c *ui.Context, p palette) {
	v := d.current()
	ui.Column(c).Grow(1).FillWidth().Children(func() {
		if v.Loading {
			ui.Row(c).Grow(1).Center().Gap(8).Children(func() { spinner(c, p); muted(c, p, tr("正在恢复会话…")) })
			return
		}
		v.List.FollowEnd = true
		v.List.Key = func(i int) any { return v.Rows[i].ID }
		ui.List(c, &v.List, len(v.Rows), func(i int) {
			row := v.Rows[i]
			ui.Column(c).FillWidth().MaxWidth(820).Margin(0, ui.Auto).Padding(0, 32).Children(func() {
				if row.Tools != nil {
					if len(row.Tools) > 1 {
						d.toolGroup(c, p, row)
					} else {
						d.tool(c, p, row.Tools[0], false)
					}
				} else {
					d.message(c, p, row.Item, i == len(v.Rows)-1)
				}
			})
		}).Grow(1).Gap(14).Padding(28, 0, 24, 0).Label(tr("对话记录")).Role(ui.RoleList)
		if !v.List.AtEnd() {
			ui.Row(c).Absolute().Bottom(14).LeftPercent(40).Children(func() {
				if button(c, p, tr("回到最新"), "ArrowDown", false).Radius(99).Background(p.Elevated).Clicked() {
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
		turn := ui.Column(c).AlignItems(ui.End).Gap(2).Margin(6, 0, 0, 0).Transition(enterMotion)
		turn.Children(func() {
			if len(item.Attachments) > 0 {
				d.attachmentChips(c, p, item.Attachments, false)
			}
			ui.Text(c, item.Text).MaxWidthPercent(85).Padding(9, 14).Radius(18).Background(p.Subtle).Border(1, p.BorderSubtle).FontSize(p.font(13.72)).LineHeight(1.6).Selectable()
			actions := ui.Row(c).Height(26).Gap(2)
			if !turn.Hovered() && !turn.FocusWithin() {
				actions.Opacity(0)
			}
			actions.Children(func() {
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
		turn := ui.Column(c).Gap(2).Transition(enterMotion)
		turn.Children(func() {
			d.markdown(c, p, item.Text)
			if item.Pending {
				pulse(c, 7, 14, 2, p.Accent, time.Second, 0, 1)
			} else {
				actions := ui.Row(c).Height(26).Gap(2)
				if !last && !turn.Hovered() && !turn.FocusWithin() {
					actions.Opacity(0)
				}
				actions.Children(func() {
					if iconButton(c, p, "Copy", tr("复制回复")).Size(26, 26).Clicked() {
						c.WriteClipboard(item.Text)
					}
					if !v.Busy && last && iconButton(c, p, "RotateCcw", tr("重新生成")).Size(26, 26).Clicked() {
						d.promptRetry()
					}
				})
			}
		})
	case "thinking":
		ui.Row(c).Height(26).Gap(10).Children(func() {
			spark(c, 14, p.Accent, true)
			ui.Text(c, tr(item.Text)).SingleLine().Grow(1).Basis(0).MinWidth(0).FontSize(p.font(12.6)).TextColor(p.Text3)
			if elapsed := time.Since(item.StartedAt); elapsed >= 2*time.Second {
				muted(c, p, fmt.Sprintf("%ds", int(elapsed.Seconds()))).NoWrap().Shrink(0).FontSize(p.font(10.92))
			}
		})
		c.After(time.Second)
	case "reasoning":
		d.reasoningView(c, p, item)
	case "notice":
		color := p.Info
		name := "Info"
		if item.Tone == "warning" {
			color = p.Warning
			name = "TriangleAlert"
		}
		if item.Tone == "error" {
			color = p.Danger
			name = "CircleAlert"
		}
		ui.Row(c).Gap(8).Padding(7, 0).Transition(enterMotion).Children(func() {
			icon(c, name, 14, color)
			ui.Text(c, localizeNotice(item.Text)).Grow(1).FontSize(p.font(11.76)).TextColor(p.Text2).Selectable()
		})
	case "summary":
		label, name, color := tr("已完成"), "CircleCheck", p.Success
		if item.Interrupted {
			label, name, color = tr("已停止"), "CircleSlash", p.Warning
		} else if !item.Success {
			label, name, color = tr("已结束"), "TriangleAlert", p.Warning
		}
		text := label + tr(" · 用时 ") + formatDuration(item.EndedAt.Sub(item.StartedAt))
		if item.Text != "" && item.Text != "0" {
			text += " · " + item.Text + tr(" 个操作")
		}
		ui.Row(c).Height(28).Gap(8).Children(func() {
			ui.Box(c).Grow(1).Height(1).Background(p.BorderSubtle)
			icon(c, name, 13, color)
			ui.Text(c, text).NoWrap().Shrink(0).FontSize(p.font(10.64)).TextColor(p.Text3)
			ui.Box(c).Grow(1).Height(1).Background(p.BorderSubtle)
		})
	case "approval":
		d.approval(c, p, item)
	}
}

func (d *desktop) reasoningView(c *ui.Context, p palette, item *model.FeedItem) {
	v := d.current()
	key := "reasoning-" + item.ID
	open, stored := v.Open[key]
	if !stored {
		open = item.Pending
	}
	root := ui.Column(c.Key(key)).FillWidth().Border(1, p.BorderSubtle).Radius(10).Background(p.Panel)
	scroll := ui.Local(root, "scroll", func() ui.ScrollState { return ui.ScrollState{} })
	root.Children(func() {
		head := ui.ButtonBase(c).Label(tr("思考过程")).Description(tr("模型接口返回的推理内容")).Expanded(open).FillWidth().Height(34).Padding(0, 12).Gap(8).Justify(ui.Start)
		if head.Hovered() {
			head.Background(p.Hover)
		}
		head.Children(func() {
			if item.Pending {
				spinner(c, p)
			} else {
				icon(c, "Sparkles", 14, p.Text3)
			}
			ui.Text(c, tr("思考过程")).NoWrap().FontSize(p.font(11.76)).FontWeight(550).TextColor(p.Text2)
			ui.Spacer(c)
			status := tr("已完成")
			if item.Pending {
				status = tr("生成中")
			} else if item.Interrupted {
				status = tr("已中断")
			}
			muted(c, p, status).NoWrap().Shrink(0).FontSize(p.font(10.36))
			icon(c, map[bool]string{true: "ChevronDown", false: "ChevronRight"}[open], 12, p.Text4)
		})
		if head.Clicked() {
			v.Open[key] = !open
		}
		if open {
			if item.Pending && scroll.Y >= scroll.MaxY-1 {
				scroll.Y = math.MaxFloat32
			}
			ui.Scroll(c).FillWidth().MaxHeight(180).TrackScroll(scroll).Padding(0, 12, 12, 12).Children(func() {
				ui.Text(c, item.Text).FillWidth().Selectable().FontSize(p.font(11.48)).LineHeight(1.6).TextColor(p.Text2)
			})
		}
	})
}

func (d *desktop) toolGroup(c *ui.Context, p palette, row model.FeedRow) {
	v := d.current()
	running := false
	counts := map[string]int{}
	failed := 0
	for _, item := range row.Tools {
		if item.Pending {
			running = true
		}
		counts[item.Name]++
		if !item.Success && !item.Pending && !item.Interrupted {
			failed++
		}
	}
	open, stored := v.Open["group-"+row.ID]
	if !stored {
		open = running || d.preferences.ExpandTools
	}
	var names []string
	for _, entry := range []struct{ Name, Format string }{{"edit", tr("编辑 %d 个文件")}, {"write", tr("写入 %d 个文件")}, {"read", tr("读取 %d 次")}, {"grep", tr("搜索 %d 次")}, {"bash", tr("运行 %d 条命令")}, {"delegate", tr("委派 %d 次")}} {
		if count := counts[entry.Name]; count > 0 {
			names = append(names, fmt.Sprintf(entry.Format, count))
		}
	}
	if failed > 0 {
		names = append(names, fmt.Sprintf(tr("%d 个失败"), failed))
	}
	group := ui.Column(c).Border(1, p.BorderSubtle).Radius(12).Background(p.Panel).Transition(fastMotion)
	if open {
		group.BorderColor(p.Border)
	}
	group.Children(func() {
		label := fmt.Sprintf(tr("已执行 %d 个操作"), len(row.Tools))
		if running {
			label = fmt.Sprintf(tr("正在执行 %d 个操作"), len(row.Tools))
		}
		head := ui.ButtonBase(c).Label(label).FillWidth().MinHeight(42).Gap(8).Radius(12).Padding(8, 12)
		if head.Hovered() {
			head.Background(p.Hover)
		}
		head.Children(func() {
			icon(c, "Layers", 14, p.Text3)
			ui.Column(c).Grow(1).Basis(0).MinWidth(0).Gap(3).Children(func() {
				ui.Text(c, label).NoWrap().FontSize(p.font(12.04)).FontWeight(550).TextColor(p.Text2)
				if len(names) > 0 {
					muted(c, p, strings.Join(names, "、")).SingleLine().FontSize(p.font(10.92))
				}
			})
			icon(c, map[bool]string{true: "ChevronDown", false: "ChevronRight"}[open], 12, p.Text4)
		})
		if head.Clicked() {
			v.Open["group-"+row.ID] = !open
		}
		if open {
			ui.Column(c).Padding(0, 6, 6, 6).Transition(enterMotion).Children(func() {
				for _, item := range row.Tools {
					d.tool(c, p, item, false)
				}
			})
		}
		if !open {
			for _, item := range row.Tools {
				if item.Success && len(item.Attachments) > 0 {
					ui.Column(c).Padding(0, 12, 12).Children(func() { d.readImages(c, p, item.Attachments) })
				}
			}
		}
	})
}

func (d *desktop) tool(c *ui.Context, p palette, item *model.FeedItem, force bool) {
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
		open = force || d.preferences.ExpandTools || (item.Pending && item.Name == "bash" && !waiting)
	}
	ui.Column(c).Children(func() {
		head := ui.ButtonBase(c).Label(meta.Label+" "+toolTarget(item)).FillWidth().Height(30).Gap(8).Padding(0, 6).Radius(8)
		if head.Hovered() {
			head.Background(p.Hover)
		}
		head.Children(func() {
			if item.Pending && !waiting {
				spinner(c, p)
			} else {
				color := p.Text3
				if !item.Success && !item.Pending && !item.Interrupted {
					color = p.Danger
				}
				icon(c, meta.Icon, 14, color)
			}
			label := meta.Done
			if item.Pending {
				label = meta.Label
			}
			ui.Text(c, label).NoWrap().Shrink(0).FontSize(p.font(11.76)).TextColor(p.Text2)
			ui.Text(c, toolTarget(item)).Grow(1).Basis(0).MinWidth(0).SingleLine().Font("Cascadia Mono, Consolas").FontSize(p.font(10.92)).TextColor(p.Text3)
			if waiting {
				badge(c, p, tr("等待批准"), p.Warning, p.WarningSoft)
			} else if item.Interrupted {
				badge(c, p, tr("已中断"), p.Warning, p.WarningSoft)
			} else if item.Auto {
				badge(c, p, tr("自动批准"), p.Success, p.SuccessSoft)
			}
			if !item.StartedAt.IsZero() && !item.EndedAt.IsZero() {
				muted(c, p, fmt.Sprintf("%.1fs", item.EndedAt.Sub(item.StartedAt).Seconds())).NoWrap().Shrink(0).FontSize(p.font(10.08))
			}
			name := "ChevronRight"
			if open {
				name = "ChevronDown"
			}
			icon(c, name, 12, p.Text4)
		})
		if head.Clicked() {
			v.Open["tool-"+item.ID] = !open
		}
		if item.Success && len(item.Attachments) > 0 {
			ui.Column(c).Margin(4, 0, 8, 20).Children(func() { d.readImages(c, p, item.Attachments) })
		}
		if open {
			ui.Column(c).Margin(4, 0, 4, 20).Padding(10, 12).Border(1, p.BorderSubtle).Radius(8).Background(p.Panel).Gap(8).Transition(enterMotion).Children(func() {
				switch item.Name {
				case "bash":
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
			})
		}
	})
}

func (d *desktop) terminal(c *ui.Context, p palette, command, output string) {
	ui.Column(c).Padding(10, 12).Radius(8).Background(p.Terminal).Gap(8).Children(func() {
		if command != "" {
			ui.RichText(c, ui.Span{Text: "$ ", Color: p.Accent}, ui.Span{Text: command, Color: p.TerminalText}).Font("Cascadia Mono, Consolas").FontSize(p.font(11.76)).Selectable()
		}
		if output != "" {
			text := output
			ui.TextAreaBase(c, &text).ReadOnly(true).Label(tr("命令输出")).Height(min(280, float32(strings.Count(output, "\n")+1)*18+12)).Font("Cascadia Mono, Consolas").FontSize(p.font(11.76)).TextColor(p.TerminalText).LineHeight(1.6)
		}
	})
}
func logText(c *ui.Context, p palette, source string, maxHeight float32) {
	text := source
	ui.TextAreaBase(c, &text).ReadOnly(true).Label(tr("工具输出")).Height(min(maxHeight, float32(strings.Count(source, "\n")+1)*18+8)).Font("Cascadia Mono, Consolas").FontSize(p.font(11.2)).TextColor(p.Text2).LineHeight(1.6)
}

func (d *desktop) approval(c *ui.Context, p palette, item *model.FeedItem) {
	if item.ApprovalID == "" {
		label := tr("已拒绝")
		name, color := "ShieldX", p.Text3
		if item.Granted {
			label = tr("已批准")
			name, color = "ShieldCheck", p.Success
		}
		ui.Row(c).Gap(8).Height(28).Children(func() {
			icon(c, name, 14, color)
			ui.Text(c, label+" · "+toolTarget(item)).SingleLine().FontSize(p.font(11.76)).TextColor(p.Text3)
		})
		return
	}
	card := ui.Column(c).Radius(12).Clip().Border(1, p.Accent.Alpha(0.55)).Background(p.AccentSofter).Transition(enterMotion).Role(ui.RoleGroup).Label(tr("需要你的批准"))
	card.Children(func() {
		ui.Row(c).Padding(14, 16).Gap(10).Children(func() {
			icon(c, "ShieldAlert", 18, p.Accent)
			ui.Column(c).Grow(1).Gap(2).Children(func() {
				ui.Text(c, tr("需要你的批准")).FontSize(p.font(12.6)).FontWeight(650)
				muted(c, p, tr("Agent 请求")+toolTarget(item)).FontSize(p.font(10.92)).SingleLine()
			})
			badge(c, p, tr("等待确认"), p.AccentText, p.AccentSoft)
		})
		ui.Column(c).Padding(0, 16, 14, 16).Children(func() {
			if item.Name == "bash" {
				d.terminal(c, p, model.String(item.Args["command"]), "")
			} else {
				ui.Text(c, model.String(item.Args["path"])).Font("Cascadia Mono, Consolas").FontSize(p.font(11.2)).TextColor(p.Text2).Margin(0, 0, 8, 0)
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
		ui.Row(c).Padding(12, 16).Gap(8).BorderWidth(1, 0, 0, 0).BorderColor(p.Border).Children(func() {
			if button(c, p, tr("批准执行"), "Check", true).Disabled(item.Deciding).Clicked() {
				d.decide(item, true, false)
			}
			if button(c, p, tr("本会话始终允许 ")+item.Name, "CheckCheck", false).Background(p.Elevated).Disabled(item.Deciding).Clicked() {
				d.decide(item, true, true)
			}
			if button(c, p, tr("拒绝"), "X", false).Disabled(item.Deciding).Clicked() {
				d.decide(item, false, false)
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
	ui.Column(c).Clip().Radius(6).Font("Cascadia Mono, Consolas").FontSize(p.font(11.76)).Children(func() {
		for _, line := range lines[:count] {
			bg, color, gutter, sign := ui.Transparent, p.Text2, p.Subtle, " "
			switch line.Type {
			case "add":
				bg, color, gutter, sign = p.AddBG, p.AddText, p.AddGutter, "+"
			case "remove":
				bg, color, gutter, sign = p.RemoveBG, p.RemoveText, p.RemoveGutter, "−"
			case "hunk":
				bg, color = p.Subtle, p.Text3
			case "meta":
				color = p.Text3
			}
			ui.Row(c).Background(bg).AlignItems(ui.Stretch).Children(func() {
				if line.Type != "meta" {
					old, new := "", ""
					if line.OldLine > 0 {
						old = fmtInt(line.OldLine)
					}
					if line.NewLine > 0 {
						new = fmtInt(line.NewLine)
					}
					ui.Text(c, old).Width(34).TextAlign(ui.End).Padding(2, 5).Background(gutter).TextColor(p.Text3).FontSize(p.font(10.36))
					ui.Text(c, new).Width(34).TextAlign(ui.End).Padding(2, 5).Background(gutter).TextColor(p.Text3).FontSize(p.font(10.36))
					ui.Text(c, sign).Width(16).TextColor(color).Padding(2, 0)
				}
				ui.Text(c, func() string {
					if line.Text == "" {
						return " "
					}
					return line.Text
				}()).Grow(1).Padding(2, 5).TextColor(color).NoWrap().Selectable().LineHeight(1.55)
			})
		}
		if count < len(lines) && button(c, p, fmt.Sprintf(tr("显示全部 %d 行"), len(lines)), "ChevronDown", false).FillWidth().Border(0, ui.Transparent).Clicked() {
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
