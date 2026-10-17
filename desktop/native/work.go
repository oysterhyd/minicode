package main

import (
	"fmt"
	"sort"
	"strings"
	"time"

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
	ui.Row(c).Height(48).Padding(8, 8, 6, 10).Gap(6).Shrink(0).Children(func() {
		if iconButton(c, p, "PanelRightClose", tr("收拢右侧面板")).Shrink(0).Clicked() {
			d.preferences.RightCollapsed = true
			d.persist()
		}
		ui.ScrollHorizontal(c).Grow(1).Basis(0).MinWidth(0).Height(34).AlignItems(ui.Center).Children(func() {
			d.workTabs(c, p)
		})
		if iconButton(c, p, "RefreshCw", tr("刷新工作区")).Shrink(0).Clicked() {
			d.refresh(v, true)
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
		ui.Row(c).Padding(10, 12, 8, 16).Gap(8).Children(func() {
			ui.Text(c, tr("未提交的改动")).Grow(1).FontSize(p.font(11.2)).FontWeight(600).TextColor(p.Text2)
			d.diffStat(c, p, a, r)
			if button(c, p, tr("全部暂存"), "Plus", false).Height(26).Radius(9).FontSize(p.font(10.64)).Padding(0, 9).Disabled(len(v.Changes) == 0).Clicked() {
				d.stage("", true, false)
			}
		})
		ui.List(c, nil, len(v.Changes), func(i int) {
			change := v.Changes[i]
			row := ui.ButtonBase(c).Label(basename(change.Path)+" "+dirname(change.Path)+" "+statusLetter(change.Status)).FillWidth().Height(46).Padding(6, 10).Gap(10).Radius(12).Cursor(ui.CursorDefault).Transition(fastMotion)
			if row.Hovered() {
				row.Background(p.Hover)
			}
			row.Children(func() {
				ui.Box(c).Size(28, 28).Shrink(0).Radius(8).Background(p.Subtle).Border(1, p.BorderSubtle).Center().Children(func() {
					icon(c, "FileCode2", 14, p.Text2)
				})
				ui.Column(c).Grow(1).Gap(2).Children(func() {
					ui.Text(c, basename(change.Path)).SingleLine().FontSize(p.font(12.04)).FontWeight(550)
					muted(c, p, dirname(change.Path)).FontSize(p.font(10.08)).SingleLine()
				})
				d.diffStat(c, p, change.Additions, change.Deletions)
				color := p.Warning
				switch statusLetter(change.Status) {
				case "U", "A":
					color = p.Success
				case "D":
					color = p.Danger
				}
				ui.Text(c, statusLetter(change.Status)).Width(14).TextAlign(ui.Center).Font(monoFont).FontSize(p.font(10.64)).FontWeight(600).TextColor(color)
			})
			if row.Clicked() {
				d.openPreview("diff", change.Path, change.Status)
			}
		}).Grow(1).Padding(0, 8).Children(func() {
			if len(v.Changes) == 0 {
				empty(c, p, tr("工作区没有未提交的改动。"))
			}
		})
	case 1:
		ui.Row(c).Height(32).Margin(4, 12, 8, 12).Padding(0, 10).Gap(7).Radius(10).Background(p.Active).Children(func() {
			icon(c, "Search", 13, p.Text3)
			ui.TextInputBase(c, &d.fileQuery).Label(tr("搜索文件")).Placeholder(tr("搜索文件")).Grow(1).FontSize(p.font(11.76))
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
		}).Grow(1).Padding(4, 12, 12, 12).Gap(10).Children(func() {
			if len(items) == 0 {
				empty(c, p, tr("Agent 运行的终端命令会显示在这里。"))
			}
		})
	case 3:
		ui.Scroll(c).Grow(1).Padding(6, 12, 12, 12).Gap(8).Children(func() {
			done := 0
			for _, task := range v.Tasks {
				if task.Status == "done" {
					done++
				}
			}
			ui.Row(c).Padding(2, 6).Children(func() {
				muted(c, p, tr("任务进度")).Grow(1).FontSize(p.font(10.36))
				muted(c, p, fmt.Sprintf("%d / %d", done, len(v.Tasks))).FontSize(p.font(10.36))
			})
			if len(v.Tasks) > 0 {
				ui.Row(c).Height(6).Margin(2, 6, 8, 6).Radius(99).Clip().Background(p.Active).Children(func() {
					ui.Box(c).WidthPercent(100 * float32(done) / float32(len(v.Tasks))).FillHeight().Radius(99).Background(p.Ink).Transition(panelMotion)
				})
			}
			if len(v.Tasks) > 0 {
				group(c, p, func() {
					for _, task := range v.Tasks {
						d.taskRow(c, p, task)
					}
				})
			}
			if len(v.Tasks) == 0 {
				empty(c, p, tr("任务开始后，这里会显示执行步骤。"))
			}
			d.sessionUsage(c, p)
		})
	}
}

// workTabs is a segmented control of the work panel's pages, with a
// raised thumb that springs to the chosen one and counts beside them.
func (d *desktop) workTabs(c *ui.Context, p palette) {
	v := d.current()
	track := ui.Row(c).Gap(1).Padding(2).Shrink(0).Radius(11).Background(p.Active).Role(ui.RoleTabList).Label(tr("工作区"))
	state := ui.Local(track, "tabs", func() segmentedState {
		return segmentedState{X: make([]float32, len(workTabs)), W: make([]float32, len(workTabs))}
	})
	if d.tab >= 0 && d.tab < len(state.W) && state.W[d.tab] > 0 {
		spring := ui.Animation{Spring: ui.SpringOptions{Duration: 340 * time.Millisecond, Bounce: 0.14}}
		x, w := track.AnimateTo("x", state.X[d.tab], spring), track.AnimateTo("w", state.W[d.tab], spring)
		track.Draw(func(painter *ui.Painter, r ui.Rect) {
			thumb := ui.Rect{X: r.X + x, Y: r.Y + 2, W: w, H: r.H - 4}
			painter.Shadow(thumb, 9, 0, 1, 3, 0, ui.RGBA(0, 0, 0, 0.12))
			painter.Fill(thumb, p.Thumb, 9)
		})
	}
	track.Children(func() {
		origin := track.Bounds()
		for i, tab := range workTabs {
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
			current := i == d.tab
			b := ui.ButtonBase(c).Label(tr(tab.Label)).Role(ui.RoleTab).Height(28).Padding(0, 10).Gap(6).Radius(9).
				FontSize(p.font(11.48)).TextColor(p.Text3).Cursor(ui.CursorDefault)
			if current {
				b.TextColor(p.Text).FontWeight(550)
				if state.W[i] == 0 {
					b.Background(p.Thumb)
				}
			} else if b.Hovered() {
				b.TextColor(p.Text2)
			}
			b.Children(func() {
				color := p.Text3
				if current {
					color = p.Text
				}
				icon(c, tab.Icon, 13, color)
				ui.Text(c, tr(tab.Label)).NoWrap()
				if count != "" {
					ui.Text(c, count).Font(monoFont).FontSize(p.font(10.08)).TextColor(p.Text3)
				}
			})
			if r := b.Bounds(); r.W > 0 {
				state.X[i], state.W[i] = r.X-origin.X, r.W
			}
			if b.Clicked() {
				d.tab = i
				d.preview = nil
				d.fileQuery = ""
			}
		}
	})
}

func empty(c *ui.Context, p palette, text string) {
	ui.Text(c, tr(text)).Padding(20).Margin(6).FontSize(p.font(11.76)).LineHeight(1.6).TextColor(p.Text3).TextAlign(ui.Center).Border(1, p.Border).BorderStyle(ui.BorderDashed).Radius(14)
}

// taskRow is one step of the plan in the tasks page's grouped list.
func (d *desktop) taskRow(c *ui.Context, p palette, task model.Task) {
	divided := settingsRows > 0
	settingsRows++
	row := ui.Row(c).FillWidth().MinHeight(30).Gap(10)
	if divided {
		row.Draw(func(painter *ui.Painter, r ui.Rect) {
			painter.Fill(ui.Rect{X: r.X + 28, Y: r.Y - 6.5, W: r.W - 28, H: 1}, p.BorderSubtle, 0)
		})
	}
	row.Children(func() {
		switch task.Status {
		case "done":
			ui.Box(c).Size(18, 18).Shrink(0).Radius(99).Background(p.Ink).Center().Children(func() { icon(c, "Check", 11, p.InkText) })
		case "running":
			ui.Box(c).Size(18, 18).Shrink(0).Center().Children(func() { spark(c, 15, p.Accent, true) })
		case "failed":
			icon(c, "CircleAlert", 18, p.Danger)
		default:
			ui.Box(c).Size(18, 18).Shrink(0).Radius(99).Border(1.5, p.BorderStrong)
		}
		text := ui.Text(c, task.Title).Grow(1).Basis(0).MinWidth(0).SingleLine().FontSize(p.font(11.9))
		switch task.Status {
		case "done":
			text.Strikethrough().TextColor(p.Text3)
		case "running":
			text.FontWeight(600)
		}
		label := map[string]string{"done": tr("完成"), "running": tr("进行中"), "failed": tr("失败"), "pending": tr("待执行")}[task.Status]
		muted(c, p, label).FontSize(p.font(10.08))
	})
}

func (d *desktop) diffStat(c *ui.Context, p palette, a, r int) {
	if a == 0 && r == 0 {
		return
	}
	ui.Row(c).Gap(4).Children(func() {
		ui.Text(c, fmt.Sprintf("+%d", a)).Font(monoFont).FontSize(p.font(10.36)).TextColor(p.Success)
		ui.Text(c, fmt.Sprintf("−%d", r)).Font(monoFont).FontSize(p.font(10.36)).TextColor(p.Danger)
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
		row := ui.ButtonBase(c).Label(entry.Path).FillWidth().Height(28).Padding(0, 8, 0, 8+float32(entry.Depth)*14).Gap(6).Radius(9).Role(ui.RoleTreeItem).Cursor(ui.CursorDefault)
		if row.Hovered() {
			row.Background(p.Hover)
		}
		row.Children(func() {
			if entry.Folder {
				disclosure(c, p, d.folders[entry.Path])
				icon(c, map[bool]string{true: "FolderOpen", false: "Folder"}[d.folders[entry.Path]], 14, p.Text3)
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
			if menu.Item(tr("在输入框中引用")).Chosen() {
				d.insertMention(entry.Path)
			}
			if menu.Item(tr("在文件夹中显示")).Chosen() {
				d.reveal(v.Workspace, entry.Path)
			}
			if menu.Item(tr("用默认应用打开")).Chosen() {
				d.openPath(v.Workspace, entry.Path)
			}
		})
	}).Grow(1).Padding(0, 8).Role(ui.RoleTree).Label(tr("工作区文件"))
}

func (d *desktop) previewView(c *ui.Context, p palette) {
	preview := d.preview
	root := d.current().Workspace
	ui.Row(c).Height(44).Padding(0, 6).Gap(4).BorderWidth(0, 0, 1, 0).BorderColor(p.BorderSubtle).Children(func() {
		if iconButton(c, p, "ArrowLeft", tr("返回文件列表")).Clicked() {
			d.preview = nil
			return
		}
		ui.Column(c).Grow(1).Margin(0, 4).Children(func() {
			ui.Text(c, basename(preview.Path)).FontSize(p.font(12.04)).FontWeight(600).SingleLine()
			muted(c, p, preview.Path).FontSize(p.font(10.08)).SingleLine()
		})
		if iconButton(c, p, "AtSign", tr("在输入框中引用")).Size(24, 24).Clicked() {
			d.insertMention(preview.Path)
		}
		if iconButton(c, p, "FolderSearch", tr("在文件夹中显示")).Size(24, 24).Clicked() {
			d.reveal(root, preview.Path)
		}
		if iconButton(c, p, "ExternalLink", tr("用默认应用打开")).Size(24, 24).Clicked() {
			d.openPath(root, preview.Path)
		}
		if iconButton(c, p, "Copy", tr("复制内容")).Size(24, 24).Clicked() {
			c.WriteClipboard(preview.Text)
		}
	})
	if preview.Loading {
		ui.Row(c).Grow(1).Padding(24).Gap(8).Children(func() { spinner(c, p); muted(c, p, tr("正在读取…")) })
		return
	}
	if preview.Error != "" {
		ui.Text(c, preview.Error).Grow(1).Padding(16).TextColor(p.Danger)
		return
	}
	if preview.Kind == "file" {
		ui.List(c, &preview.List, len(preview.Lines), func(i int) {
			ui.Row(c).Gap(0).AlignItems(ui.Stretch).Children(func() {
				ui.Text(c, fmtInt(i+1)).Width(42).Padding(0, 10, 0, 0).TextAlign(ui.End).Font(monoFont).FontSize(p.font(10.92)).TextColor(p.Text4)
				codeText(c, p, preview.Lines[i])
			})
		}).Grow(1).Padding(6, 0).Background(p.Main)
	} else {
		ui.List(c, &preview.List, len(preview.Diff), func(i int) { d.diffView(c, p, preview.Diff[i:i+1], "preview-row", 0) }).Grow(1).Background(p.Main)
		if len(preview.Diff) == 0 {
			muted(c, p, preview.Text).Grow(1).Padding(12)
		}
		staged := len(preview.Status) > 0 && preview.Status[0] != ' ' && preview.Status[0] != '?'
		ui.Row(c).Padding(8, 10).Gap(6).BorderWidth(1, 0, 0, 0).BorderColor(p.BorderSubtle).Children(func() {
			if staged {
				badge(c, p, tr("已暂存"), p.Success, p.SuccessSoft)
			}
			ui.Spacer(c)
			if staged && button(c, p, tr("取消暂存"), "Minus", false).Disabled(preview.Busy).Clicked() {
				d.stage(preview.Path, false, true)
			}
			if button(c, p, tr("暂存此文件"), "Plus", true).Disabled(preview.Busy).Clicked() {
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
		muted(c, p, tr("会话统计")).Grow(1).FontSize(p.font(10.36))
		muted(c, p, fmt.Sprintf(tr("%d 次请求"), requests)).FontSize(p.font(10.36))
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
	ui.Row(c).Height(6).Margin(2, 0, 2, 0).Radius(99).Clip().Background(p.Active).Label(tr("输入与输出 token 分布")).Children(func() {
		if known && usage.Input+usage.Output > 0 {
			ui.Box(c).WidthPercent(inputRatio * 100).FillHeight().Background(p.Ink)
			ui.Box(c).Grow(1).FillHeight().Background(p.Accent)
		}
	})
	values := []struct{ Label, Value string }{{tr("输入 token"), tr("未报告")}, {tr("输出 token"), tr("未报告")}, {tr("缓存命中率"), tr("未报告")}, {tr("平均生成 TPS"), tr("未记录")}}
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
				ui.Column(c).Grow(1).Basis(0).Padding(10, 12).Radius(12).Border(1, p.BorderSubtle).Background(p.Elevated).Gap(2).Children(func() {
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
	ui.Row(c).Height(6).Margin(2, 0).Radius(99).Clip().Background(p.Active).Label(tr("缓存命中率")).Children(func() {
		ui.Box(c).WidthPercent(cacheRatio * 100).FillHeight().Background(p.Text3)
	})
	read, write := "—", "—"
	if known {
		read, write = number(usage.CacheRead), number(usage.CacheWrite)
	}
	for _, pair := range [][2]string{{fmt.Sprintf(tr("轮次 %d"), state.Rounds), fmt.Sprintf(tr("工具调用 %d"), calls)}, {tr("缓存读取 ") + read, tr("缓存写入 ") + write}} {
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
		muted(c, p, tr("每轮 token")).Padding(6).FontSize(p.font(10.64))
		ui.Row(c).Height(110).Gap(3).Padding(6).AlignItems(ui.End).Children(func() {
			for _, sample := range samples {
				ui.Column(c).Grow(1).Basis(0).Height(100).Justify(ui.End).Gap(2).Children(func() {
					bar := ui.Column(c).Grow(1).Justify(ui.End).Tooltip(fmt.Sprintf(tr("#%d · 输入 %d · 缓存 %d · 输出 %d"), sample.Round, sample.Input, sample.Cached, sample.Output))
					bar.Children(func() {
						ui.Box(c).Height(80 * float32(sample.Output) / float32(maxTokens)).Background(p.Info)
						ui.Box(c).Height(80 * float32(min(sample.Input, sample.Cached)) / float32(maxTokens)).Background(p.Success)
						ui.Box(c).Height(80 * float32(max(0, sample.Input-sample.Cached)) / float32(maxTokens)).Background(p.Accent)
					})
					ui.Text(c, fmtInt(sample.Round)).TextAlign(ui.Center).FontSize(p.font(9.24)).TextColor(p.Text4)
				})
			}
		})
		muted(c, p, tr("输出速度")).Padding(6).FontSize(p.font(10.64))
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
		if button(c, p, fmt.Sprintf(tr("请求明细（最近 %d 次）"), len(samples)), "ChevronDown", false).FillWidth().Border(0, ui.Transparent).Clicked() {
			v.Open["usage-table"] = !open
		}
		if open {
			for _, sample := range samples {
				speed := "—"
				if sample.TPS != nil {
					speed = fmt.Sprintf("%.1f", *sample.TPS)
				}
				ui.Text(c, fmt.Sprintf(tr("#%d  输入 %d  输出 %d  缓存 %d  TPS %s"), sample.Round, sample.Input, sample.Output, sample.Cached, speed)).FontSize(p.font(10.36)).Padding(4, 8).Selectable()
				if sample.FirstTokenSeconds != nil && sample.GenerationSeconds != nil {
					muted(c, p, fmt.Sprintf(tr("首字等待 %.2fs · 生成 %.2fs"), *sample.FirstTokenSeconds, *sample.GenerationSeconds)).FontSize(p.font(10.08)).Padding(0, 8, 4, 8)
				}
			}
		}
	}
	if stats != nil && stats.RequestTPS != nil {
		muted(c, p, fmt.Sprintf(tr("请求整体速度（含首字等待）：%.1f tok/s"), *stats.RequestTPS)).Padding(8).FontSize(p.font(10.08))
	}
	muted(c, p, tr("生成 TPS = 输出 token / 首个至最后一个生成片段的时间（包含推理与工具参数）。历史记录和单个片段无法计算生成速度；客户端测量可能与网关不同。")).Padding(8).FontSize(p.font(10.08))
}

var _ = content.DiffLine{}
