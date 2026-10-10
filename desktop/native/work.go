package main

import (
	"fmt"
	"sort"
	"strings"

	"github.com/egoist/mygo/ui"
	"minicode.desktop/internal/content"
	"minicode.desktop/internal/model"
)

var workTabs = []struct{ Label, Icon string }{{"改动", "FileDiff"}, {"文件", "Files"}, {"终端", "Terminal"}, {"任务", "ListTodo"}}

func statusLetter(status string) string {
	if status == "??" {
		return "U"
	}
	for _, letter := range []string{"D", "A", "R"} {
		if strings.Contains(status, letter) {
			return letter
		}
	}
	return "M"
}

func (d *desktop) workPanel(c *ui.Context, p palette) {
	v := d.current()
	ui.Row(c).Height(42).Padding(6).Gap(2).BorderWidth(0, 0, 1, 0).BorderColor(p.Border).Children(func() {
		for i, tab := range workTabs {
			b := button(c, p, tab.Label, tab.Icon, false).Label(tab.Label).Height(28).Padding(0, 8).FontSize(p.font(11.2)).Border(0, ui.Transparent)
			b.Role(ui.RoleTab)
			b.Children(func() {
				count := ""
				if i == 0 && len(v.Changes) > 0 {
					count = fmtInt(len(v.Changes))
				} else if i == 3 && len(v.Tasks) > 0 {
					done := 0
					for _, task := range v.Tasks {
						if task.Status == "done" {
							done++
						}
					}
					count = fmt.Sprintf("%d/%d", done, len(v.Tasks))
				}
				if count != "" {
					badge(c, p, count, p.AccentText, p.AccentSoft)
				}
			})
			if i == d.tab {
				b.Background(p.Active).TextColor(p.Text)
			}
			if b.Clicked() {
				d.tab = i
				d.preview = nil
				d.fileQuery = ""
			}
		}
		ui.Spacer(c)
		if iconButton(c, p, "RefreshCw", "刷新工作区").Size(24, 24).Clicked() {
			d.refresh(v, true)
		}
		if iconButton(c, p, "PanelRightClose", "收拢右侧面板").Size(24, 24).Clicked() {
			d.preferences.RightCollapsed = true
			d.persist()
		}
	})
	if d.preview != nil {
		d.previewView(c, p)
		return
	}
	switch d.tab {
	case 0:
		a, r := 0, 0
		for _, change := range v.Changes {
			a += change.Additions
			r += change.Deletions
		}
		ui.Row(c).Padding(10, 10, 8, 12).Gap(6).Children(func() {
			muted(c, p, "未提交的改动").Grow(1).FontSize(p.font(10.36))
			d.diffStat(c, p, a, r)
			if button(c, p, "全部暂存", "Plus", false).Height(26).FontSize(p.font(10.64)).Padding(0, 8).Disabled(len(v.Changes) == 0).Clicked() {
				d.stage("", true, false)
			}
		})
		ui.List(c, nil, len(v.Changes), func(i int) {
			change := v.Changes[i]
			row := ui.ButtonBase(c).Label(basename(change.Path)+" "+dirname(change.Path)+" "+statusLetter(change.Status)).FillWidth().Height(44).Padding(6, 10).Gap(8).Radius(8)
			if row.Hovered() {
				row.Background(p.Hover)
			}
			row.Children(func() {
				icon(c, "FileCode2", 14, p.Text3)
				ui.Column(c).Grow(1).Gap(2).Children(func() {
					ui.Text(c, basename(change.Path)).SingleLine().FontSize(p.font(12.04)).FontWeight(550)
					muted(c, p, dirname(change.Path)).FontSize(p.font(10.08)).SingleLine()
				})
				d.diffStat(c, p, change.Additions, change.Deletions)
				color, bg := p.Warning, p.WarningSoft
				if change.Status == "??" {
					color, bg = p.Success, p.SuccessSoft
				}
				badge(c, p, statusLetter(change.Status), color, bg)
			})
			if row.Clicked() {
				d.openPreview("diff", change.Path, change.Status)
			}
		}).Grow(1).Padding(0, 6).Children(func() {
			if len(v.Changes) == 0 {
				empty(c, p, "工作区没有未提交的改动。")
			}
		})
	case 1:
		ui.Row(c).Height(40).Padding(6, 10).Gap(7).Children(func() {
			icon(c, "Search", 13, p.Text3)
			ui.TextInputBase(c, &d.fileQuery).Label("搜索文件").Placeholder("搜索文件").Grow(1).FontSize(p.font(11.76))
		})
		d.fileTree(c, p)
	case 2:
		var items []*model.FeedItem
		for _, item := range v.Items {
			if item.Kind == "tool" && item.Name == "bash" {
				items = append(items, item)
			}
		}
		ui.List(c, nil, len(items), func(i int) {
			item := items[i]
			d.terminal(c, p, model.String(item.Args["command"]), item.Output)
		}).Grow(1).Padding(10).Gap(8).Children(func() {
			if len(items) == 0 {
				empty(c, p, "Agent 运行的终端命令会显示在这里。")
			}
		})
	case 3:
		ui.Scroll(c).Grow(1).Padding(10, 8).Gap(8).Children(func() {
			done := 0
			for _, task := range v.Tasks {
				if task.Status == "done" {
					done++
				}
			}
			ui.Row(c).Padding(2, 6).Children(func() {
				muted(c, p, "任务进度").Grow(1).FontSize(p.font(10.36))
				muted(c, p, fmt.Sprintf("%d / %d", done, len(v.Tasks))).FontSize(p.font(10.36))
			})
			if len(v.Tasks) > 0 {
				ui.Row(c).Height(4).Margin(2, 8, 8, 8).Radius(99).Clip().Background(p.Active).Children(func() {
					ui.Box(c).WidthPercent(100 * float32(done) / float32(len(v.Tasks))).FillHeight().Background(p.Success)
				})
			}
			for _, task := range v.Tasks {
				ui.Row(c).Height(32).Padding(4, 8).Gap(9).Children(func() {
					name, color := "CircleDashed", p.Text3
					switch task.Status {
					case "done":
						name, color = "CircleCheck", p.Success
					case "running":
						color = p.Accent
					case "failed":
						name, color = "CircleAlert", p.Danger
					}
					icon(c, name, 17, color)
					text := ui.Text(c, task.Title).Grow(1).SingleLine().FontSize(p.font(11.9))
					if task.Status == "done" {
						text.Strikethrough().TextColor(p.Text3)
					}
					label := map[string]string{"done": "完成", "running": "进行中", "failed": "失败", "pending": "待执行"}[task.Status]
					muted(c, p, label).FontSize(p.font(10.08))
				})
			}
			if len(v.Tasks) == 0 {
				empty(c, p, "任务开始后，这里会显示执行步骤。")
			}
			d.sessionUsage(c, p)
		})
	}
}

func empty(c *ui.Context, p palette, text string) {
	ui.Text(c, text).Padding(18).Margin(6).FontSize(p.font(11.76)).TextColor(p.Text3).TextAlign(ui.Center).Border(1, p.Border).BorderStyle(ui.BorderDashed).Radius(8)
}

func (d *desktop) diffStat(c *ui.Context, p palette, a, r int) {
	if a == 0 && r == 0 {
		return
	}
	ui.Row(c).Gap(4).Children(func() {
		ui.Text(c, fmt.Sprintf("+%d", a)).Font("Cascadia Mono, Consolas").FontSize(p.font(10.08)).TextColor(p.Success)
		ui.Text(c, fmt.Sprintf("−%d", r)).Font("Cascadia Mono, Consolas").FontSize(p.font(10.08)).TextColor(p.Danger)
	})
}

type treeEntry struct {
	Name, Path string
	Folder     bool
	Depth      int
}

type treeCache struct {
	Key, Query                   string
	FilesVersion, FolderRevision uint64
	Entries                      []treeEntry
}

func (d *desktop) fileTree(c *ui.Context, p palette) {
	v := d.current()
	query := strings.ToLower(strings.TrimSpace(d.fileQuery))
	if cache := d.treeCache; cache != nil && cache.Key == v.Key && cache.Query == query && cache.FilesVersion == v.FilesVersion && cache.FolderRevision == d.folderRevision {
		d.fileTreeEntries(c, p, cache.Entries)
		return
	}
	entries := []treeEntry{}
	if query != "" {
		for _, path := range v.Files {
			if strings.Contains(strings.ToLower(path), query) {
				entries = append(entries, treeEntry{basename(path), path, false, 0})
			}
		}
	} else {
		tree := map[string][]treeEntry{}
		seen := map[string]bool{}
		for _, path := range v.Files {
			parts := strings.Split(path, "/")
			for i := range parts {
				nodePath := strings.Join(parts[:i+1], "/")
				if seen[nodePath] {
					continue
				}
				seen[nodePath] = true
				parent := strings.Join(parts[:i], "/")
				tree[parent] = append(tree[parent], treeEntry{parts[i], nodePath, i < len(parts)-1, i})
			}
		}
		var add func(string)
		add = func(parent string) {
			children := tree[parent]
			sort.Slice(children, func(i, j int) bool {
				if children[i].Folder != children[j].Folder {
					return children[i].Folder
				}
				return strings.ToLower(children[i].Name) < strings.ToLower(children[j].Name)
			})
			for _, entry := range children {
				entries = append(entries, entry)
				if entry.Folder && d.folders[entry.Path] {
					add(entry.Path)
				}
			}
		}
		add("")
	}
	d.treeCache = &treeCache{v.Key, query, v.FilesVersion, d.folderRevision, entries}
	d.fileTreeEntries(c, p, entries)
}

func (d *desktop) fileTreeEntries(c *ui.Context, p palette, entries []treeEntry) {
	v := d.current()
	ui.List(c, nil, len(entries), func(i int) {
		entry := entries[i]
		row := ui.ButtonBase(c).Label(entry.Path).FillWidth().Height(28).Padding(0, 8, 0, 8+float32(entry.Depth)*14).Gap(6).Radius(6).Role(ui.RoleTreeItem)
		if row.Hovered() {
			row.Background(p.Hover)
		}
		row.Children(func() {
			if entry.Folder {
				name := "ChevronRight"
				if d.folders[entry.Path] {
					name = "ChevronDown"
				}
				icon(c, name, 12, p.Text4)
				icon(c, "Folder", 14, p.Text3)
			} else {
				ui.Box(c).Width(12)
				icon(c, "FileCode2", 14, p.Text3)
			}
			ui.Text(c, entry.Name).Grow(1).SingleLine().FontSize(p.font(11.76))
		})
		if row.Clicked() {
			if entry.Folder {
				d.folders[entry.Path] = !d.folders[entry.Path]
				d.folderRevision++
			} else {
				d.openPreview("file", entry.Path, "")
			}
		}
		row.ContextMenu(func(menu *ui.Menu) {
			if menu.Item("在输入框中引用").Chosen() {
				d.insertMention(entry.Path)
			}
			if menu.Item("在文件夹中显示").Chosen() {
				d.reveal(v.Workspace, entry.Path)
			}
			if menu.Item("用默认应用打开").Chosen() {
				d.openPath(v.Workspace, entry.Path)
			}
		})
	}).Grow(1).Padding(0, 6).Role(ui.RoleTree).Label("工作区文件")
}

func (d *desktop) previewView(c *ui.Context, p palette) {
	preview := d.preview
	root := d.current().Workspace
	ui.Row(c).Height(44).Padding(0, 6).Gap(4).BorderWidth(0, 0, 1, 0).BorderColor(p.BorderSubtle).Children(func() {
		if iconButton(c, p, "ArrowLeft", "返回文件列表").Clicked() {
			d.preview = nil
			return
		}
		ui.Column(c).Grow(1).Margin(0, 4).Children(func() {
			ui.Text(c, basename(preview.Path)).FontSize(p.font(12.04)).FontWeight(600).SingleLine()
			muted(c, p, preview.Path).FontSize(p.font(10.08)).SingleLine()
		})
		if iconButton(c, p, "AtSign", "在输入框中引用").Size(24, 24).Clicked() {
			d.insertMention(preview.Path)
		}
		if iconButton(c, p, "FolderSearch", "在文件夹中显示").Size(24, 24).Clicked() {
			d.reveal(root, preview.Path)
		}
		if iconButton(c, p, "ExternalLink", "用默认应用打开").Size(24, 24).Clicked() {
			d.openPath(root, preview.Path)
		}
		if iconButton(c, p, "Copy", "复制内容").Size(24, 24).Clicked() {
			c.WriteClipboard(preview.Text)
		}
	})
	if preview.Loading {
		ui.Row(c).Grow(1).Padding(24).Gap(8).Children(func() { spinner(c, p); muted(c, p, "正在读取…") })
		return
	}
	if preview.Error != "" {
		ui.Text(c, preview.Error).Grow(1).Padding(16).TextColor(p.Danger)
		return
	}
	if preview.Kind == "file" {
		ui.List(c, &preview.List, len(preview.Lines), func(i int) {
			ui.Row(c).Gap(0).AlignItems(ui.Stretch).Children(func() {
				ui.Text(c, fmtInt(i+1)).Width(42).Padding(0, 10, 0, 0).TextAlign(ui.End).Font("Cascadia Mono, Consolas").FontSize(p.font(10.92)).TextColor(p.Text4)
				codeText(c, p, preview.Lines[i])
			})
		}).Grow(1).Padding(6, 0).Background(p.Main).HorizontalScroll()
	} else {
		ui.List(c, &preview.List, len(preview.Diff), func(i int) { d.diffView(c, p, preview.Diff[i:i+1], "preview-row", 0) }).Grow(1).Background(p.Main).HorizontalScroll()
		if len(preview.Diff) == 0 {
			muted(c, p, preview.Text).Grow(1).Padding(12)
		}
		staged := len(preview.Status) > 0 && preview.Status[0] != ' ' && preview.Status[0] != '?'
		ui.Row(c).Padding(8, 10).Gap(6).BorderWidth(1, 0, 0, 0).BorderColor(p.BorderSubtle).Children(func() {
			if staged {
				badge(c, p, "已暂存", p.Success, p.SuccessSoft)
			}
			ui.Spacer(c)
			if staged && button(c, p, "取消暂存", "Minus", false).Disabled(preview.Busy).Clicked() {
				d.stage(preview.Path, false, true)
			}
			if button(c, p, "暂存此文件", "Plus", true).Disabled(preview.Busy).Clicked() {
				d.stage(preview.Path, false, false)
			}
		})
	}
}

func (d *desktop) sessionUsage(c *ui.Context, p palette) {
	v := d.current()
	state := v.State
	usage := state.Usage
	stats := state.Statistics
	requests := 0
	if stats != nil {
		requests = stats.Requests
	}
	ui.Row(c).Padding(6).Children(func() {
		muted(c, p, "会话统计").Grow(1).FontSize(p.font(10.36))
		muted(c, p, fmt.Sprintf("%d 次请求", requests)).FontSize(p.font(10.36))
	})
	known := usage != nil && usage.Available
	inputRatio, cacheRatio := float32(0), float32(0)
	if known {
		if total := usage.Input + usage.Output; total > 0 {
			inputRatio = float32(usage.Input) / float32(total)
		}
		if usage.Input > 0 {
			cacheRatio = min(1, float32(usage.CacheRead)/float32(usage.Input))
		}
	}
	ui.Row(c).Height(6).Margin(2, 0, 2, 0).Radius(99).Clip().Background(p.Active).Label("输入与输出 token 分布").Children(func() {
		if known && usage.Input+usage.Output > 0 {
			ui.Box(c).WidthPercent(inputRatio * 100).FillHeight().Background(p.Accent)
			ui.Box(c).Grow(1).FillHeight().Background(p.Success)
		}
	})
	values := []struct{ Label, Value string }{{"输入 token", "未报告"}, {"输出 token", "未报告"}, {"缓存命中率", "未报告"}, {"平均生成 TPS", "未记录"}}
	if usage != nil && usage.Available {
		values[0].Value = number(usage.Input)
		values[1].Value = number(usage.Output)
		if usage.Input > 0 {
			values[2].Value = fmt.Sprintf("%.1f%%", cacheRatio*100)
		}
	}
	if stats != nil && stats.TPS != nil {
		values[3].Value = fmt.Sprintf("%.1f tok/s", *stats.TPS)
	}
	for pair := 0; pair < 2; pair++ {
		ui.Row(c).Gap(6).Padding(0, 6).Children(func() {
			for _, value := range values[pair*2 : pair*2+2] {
				ui.Column(c).Grow(1).Basis(0).Padding(10, 12).Radius(8).Border(1, p.BorderSubtle).Background(p.Main).Gap(2).Children(func() {
					muted(c, p, value.Label).FontSize(p.font(10.08))
					ui.Text(c, value.Value).FontSize(p.font(14.7)).FontWeight(600)
				})
			}
		})
	}
	calls := 0
	if stats != nil {
		calls = stats.ToolCalls
	}
	ui.Row(c).Height(6).Margin(2, 0).Radius(99).Clip().Background(p.Active).Label("缓存命中率").Children(func() {
		ui.Box(c).WidthPercent(cacheRatio * 100).FillHeight().Background(p.Text3)
	})
	read, write := "—", "—"
	if known {
		read, write = number(usage.CacheRead), number(usage.CacheWrite)
	}
	for _, pair := range [][2]string{{fmt.Sprintf("轮次 %d", state.Rounds), fmt.Sprintf("工具调用 %d", calls)}, {"缓存读取 " + read, "缓存写入 " + write}} {
		ui.Row(c).Padding(0, 6).Children(func() {
			muted(c, p, pair[0]).Grow(1).Basis(0).FontSize(p.font(10.5))
			muted(c, p, pair[1]).Grow(1).Basis(0).FontSize(p.font(10.5))
		})
	}
	if stats != nil && len(stats.Samples) > 0 {
		samples := stats.Samples[max(0, len(stats.Samples)-32):]
		maxTokens, maxTPS := 1, 1.0
		for _, sample := range samples {
			maxTokens = max(maxTokens, sample.Input+sample.Output)
			if sample.TPS != nil {
				maxTPS = max(maxTPS, *sample.TPS)
			}
		}
		muted(c, p, "每轮 token").Padding(6).FontSize(p.font(10.64))
		ui.Row(c).Height(110).Gap(3).Padding(6).AlignItems(ui.End).Children(func() {
			for _, sample := range samples {
				ui.Column(c).Grow(1).Basis(0).Height(100).Justify(ui.End).Gap(2).Children(func() {
					bar := ui.Column(c).Grow(1).Justify(ui.End).Tooltip(fmt.Sprintf("#%d · 输入 %d · 缓存 %d · 输出 %d", sample.Round, sample.Input, sample.Cached, sample.Output))
					bar.Children(func() {
						ui.Box(c).Height(80 * float32(sample.Output) / float32(maxTokens)).Background(p.Info)
						ui.Box(c).Height(80 * float32(min(sample.Input, sample.Cached)) / float32(maxTokens)).Background(p.Success)
						ui.Box(c).Height(80 * float32(max(0, sample.Input-sample.Cached)) / float32(maxTokens)).Background(p.Accent)
					})
					ui.Text(c, fmtInt(sample.Round)).TextAlign(ui.Center).FontSize(p.font(9.24)).TextColor(p.Text4)
				})
			}
		})
		muted(c, p, "输出速度").Padding(6).FontSize(p.font(10.64))
		ui.Box(c).Height(68).Margin(0, 6).Draw(func(painter *ui.Painter, r ui.Rect) {
			var path ui.Path
			started := false
			for i, sample := range samples {
				if sample.TPS == nil {
					continue
				}
				x := r.X + 4 + float32(i)*(r.W-8)/float32(max(1, len(samples)-1))
				y := r.Y + 60 - float32(*sample.TPS/maxTPS)*52
				if !started {
					path.MoveTo(x, y)
					started = true
				} else {
					path.LineTo(x, y)
				}
			}
			if started {
				painter.StrokePath(&path, 2, p.Accent)
			}
		})
		open := v.Open["usage-table"]
		if button(c, p, fmt.Sprintf("请求明细（最近 %d 次）", len(samples)), "ChevronDown", false).FillWidth().Border(0, ui.Transparent).Clicked() {
			v.Open["usage-table"] = !open
		}
		if open {
			for _, sample := range samples {
				speed := "—"
				if sample.TPS != nil {
					speed = fmt.Sprintf("%.1f", *sample.TPS)
				}
				ui.Text(c, fmt.Sprintf("#%d  输入 %d  输出 %d  缓存 %d  TPS %s", sample.Round, sample.Input, sample.Output, sample.Cached, speed)).FontSize(p.font(10.36)).Padding(4, 8).Selectable()
				if sample.FirstTokenSeconds != nil && sample.GenerationSeconds != nil {
					muted(c, p, fmt.Sprintf("首字等待 %.2fs · 生成 %.2fs", *sample.FirstTokenSeconds, *sample.GenerationSeconds)).FontSize(p.font(10.08)).Padding(0, 8, 4, 8)
				}
			}
		}
	}
	if stats != nil && stats.RequestTPS != nil {
		muted(c, p, fmt.Sprintf("请求整体速度（含首字等待）：%.1f tok/s", *stats.RequestTPS)).Padding(8).FontSize(p.font(10.08))
	}
	muted(c, p, "生成 TPS = 输出 token / 首个至最后一个生成片段的时间（包含推理与工具参数）。历史记录和单个片段无法计算生成速度；客户端测量可能与网关不同。").Padding(8).FontSize(p.font(10.08))
}

var _ = content.DiffLine{}
