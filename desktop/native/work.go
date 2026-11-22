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
	ui.Row(c).MinHeight(46).Padding(8, 8, 6, 8).Gap(4).Shrink(0).BorderWidth(0, 0, 1, 0).BorderColor(p.BorderSubtle).Children(func() {
		d.workTabs(c, p)
		ui.Spacer(c)
		refresh := iconButton(c, p, "RefreshCw", tr("刷新工作区")).Shrink(0)
		if refresh.Clicked() {
			d.refresh(v, true)
		}
		collapse := iconButton(c, p, "PanelRightClose", tr("收拢右侧面板")).Shrink(0)
		if collapse.Clicked() {
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
		a, r, staged := 0, 0, 0
		for _, change := range v.Changes {
			a += change.Additions
			r += change.Deletions
			if len(change.Status) > 0 && change.Status[0] != ' ' && change.Status[0] != '?' {
				staged++
			}
		}
		gitCard := ui.Column(c).Shrink(0).Margin(10, 10, 4, 10).Padding(10, 12).Gap(8).Radius(10).Background(p.Elevated).Border(1, p.Border)
		addSpecularTop(gitCard, p, 10)
		gitCard.Children(func() {
			ui.Row(c).Gap(6).Children(func() {
				icon(c, "GitBranch", 12, p.Accent)
				branchLabel := v.State.Branch
				if branchLabel == "" {
					branchLabel = tr("本地工作区")
				}
				ui.Text(c, branchLabel).Font(monoFont).FontSize(p.font(fontSmall)).FontWeight(600).TextColor(p.Text).SingleLine().Shrink(1).MinWidth(0)
				ui.Spacer(c)
				if staged > 0 {
					badge(c, p, trf("%d 个已暂存", staged), p.Success, p.SuccessSoft).Font(monoFont)
				} else {
					badge(c, p, tr("未暂存"), p.Text3, p.Subtle).Font(monoFont)
				}
			})
			ui.Row(c).Gap(6).Children(func() {
				ui.Text(c, trf("%d 个变更文件", len(v.Changes))).Font(monoFont).FontSize(p.font(fontCaption)).TextColor(p.Text3)
				ui.Spacer(c)
				d.diffStat(c, p, a, r)
			})
			ui.Row(c).Gap(6).Children(func() {
				if button(c, p, tr("刷新改动"), "RefreshCw", false).Grow(1).Basis(0).Height(28).Radius(6).FontSize(p.font(fontCaption)).Justify(ui.Center).Clicked() {
					d.refresh(v, true)
				}
				if button(c, p, tr("全部暂存"), "Plus", true).Grow(1).Basis(0).Height(28).Radius(6).FontSize(p.font(fontCaption)).Justify(ui.Center).Disabled(len(v.Changes) == 0).Clicked() {
					d.stage("", true, false)
				}
			})
		})
		ui.Row(c).Shrink(0).Padding(8, 12, 4, 14).Gap(8).Children(func() {
			ui.Text(c, tr("未提交的改动")).Grow(1).FontSize(p.font(fontCaption)).FontWeight(600).TextColor(p.Text3)
			if len(v.Changes) > 0 {
				ui.Text(c, fmtInt(len(v.Changes))).Font(monoFont).FontSize(p.font(fontMicro)).TextColor(p.Text3)
			}
		})
		ui.List(c, nil, len(v.Changes), func(i int) {
			change := v.Changes[i]
			letter := statusLetter(change.Status)
			row := ui.ButtonBase(c).Label(basename(change.Path)+" "+dirname(change.Path)+" "+letter).FillWidth().Height(42).Padding(4, 10).Gap(9).Radius(7).Cursor(ui.CursorDefault).Transition(fastMotion)
			if row.Hovered() {
				row.Background(p.Hover)
			}
			row.Children(func() {
				color, bg := p.Warning, p.WarningSoft
				switch letter {
				case "U", "A":
					color, bg = p.Success, p.SuccessSoft
				case "D":
					color, bg = p.Danger, p.DangerSoft
				}
				ui.Box(c).Size(20, 20).Shrink(0).Radius(5).Background(bg).Center().Children(func() {
					ui.Text(c, letter).Font(monoFont).FontSize(p.font(fontMicro)).FontWeight(700).TextColor(color)
				})
				ui.Column(c).Grow(1).Basis(0).MinWidth(0).Gap(1).Children(func() {
					ui.Text(c, basename(change.Path)).SingleLine().FontSize(p.font(fontSmall)).FontWeight(550).TextColor(p.Text)
					ui.Text(c, dirname(change.Path)).SingleLine().Font(monoFont).FontSize(p.font(fontMicro)).TextColor(p.Text4)
				})
				d.diffStat(c, p, change.Additions, change.Deletions)
			})
			if row.Clicked() {
				d.openPreview("diff", change.Path, change.Status)
			}
		}).Grow(1).Basis(0).MinHeight(0).Padding(0, 8, 8, 8).Children(func() {
			if len(v.Changes) == 0 {
				empty(c, p, tr("工作区没有未提交的改动。"))
			}
		})
	case 1:
		ui.Row(c).Height(32).Shrink(0).Margin(8, 10, 6, 10).Padding(0, 10).Gap(7).Radius(7).Background(p.Subtle).Border(1, p.BorderSubtle).Children(func() {
			icon(c, "Search", 13, p.Text3)
			ui.TextInputBase(c, &d.fileQuery).Label(tr("搜索文件")).Placeholder(tr("搜索文件")).Grow(1).FontSize(p.font(fontSmall))
		})
		d.fileTree(c, p)
	case 2:
		var items []*model.FeedItem
		for _, item := range v.Items {
			if item.Kind == "tool" && (item.Name == "bash" || item.Name == "powershell") {
				items = append(items, item)
			}
		}
		ui.List(c, nil, len(items), func(i int) {
			item := items[i]
			d.terminal(c, p, model.String(item.Args["command"]), item.Output)
		}).Grow(1).Basis(0).MinHeight(0).Padding(8, 12, 12, 12).Gap(10).Children(func() {
			if len(items) == 0 {
				empty(c, p, tr("Agent 运行的终端命令会显示在这里。"))
			}
		})
	case 3:
		ui.Scroll(c).Grow(1).Basis(0).MinHeight(0).Padding(8, 12, 12, 12).Gap(8).Children(func() {
			done := 0
			for _, task := range v.Tasks {
				if task.Status == "done" {
					done++
				}
			}
			ui.Row(c).Padding(2, 6).Children(func() {
				muted(c, p, tr("任务进度")).Grow(1).FontSize(p.font(fontCaption))
				ui.Text(c, fmt.Sprintf("%d / %d", done, len(v.Tasks))).Font(monoFont).FontSize(p.font(fontCaption)).TextColor(p.Text3)
			})
			if len(v.Tasks) > 0 {
				ui.Row(c).Height(6).Margin(2, 6, 8, 6).Radius(99).Clip().Background(p.Active).Children(func() {
					ui.Box(c).WidthPercent(100 * float32(done) / float32(len(v.Tasks))).FillHeight().Radius(99).Background(p.Accent).Transition(panelMotion)
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
	d.telemetryBento(c, p)
}

func (d *desktop) telemetryBento(c *ui.Context, p palette) {
	v := d.current()
	usage := v.State.Usage
	stats := v.State.Statistics

	inText, cacheText := "—", ""
	if usage != nil && usage.Available {
		inText = number(usage.Input)
		if usage.Input > 0 {
			pct := min(100, float64(usage.CacheRead)/float64(usage.Input)*100)
			cacheText = fmt.Sprintf("%.0f%%", pct)
		}
	}

	tpsText, tpsUnit := "—", ""
	if stats != nil {
		if stats.LastTPS != nil {
			tpsText, tpsUnit = fmt.Sprintf("%.1f", *stats.LastTPS), "tok/s"
		} else if stats.TPS != nil {
			tpsText, tpsUnit = fmt.Sprintf("%.1f", *stats.TPS), "tok/s"
		}
	}

	turnText := "—"
	if stats != nil {
		var secs float64
		if len(stats.Samples) > 0 {
			last := stats.Samples[len(stats.Samples)-1]
			if last.Seconds != nil && *last.Seconds > 0 {
				secs = *last.Seconds
			} else if last.GenerationSeconds != nil {
				secs = *last.GenerationSeconds
				if last.FirstTokenSeconds != nil {
					secs += *last.FirstTokenSeconds
				}
			}
		} else if stats.ModelSeconds > 0 {
			secs = stats.ModelSeconds / float64(max(1, stats.Requests))
		}
		if secs > 0 {
			if secs >= 1 {
				turnText = fmt.Sprintf("%.1fs", secs)
			} else {
				turnText = fmt.Sprintf("%dms", int(secs*1000))
			}
		}
	}

	add, del := 0, 0
	for _, ch := range v.Changes {
		add += ch.Additions
		del += ch.Deletions
	}

	cell := func(label string, body func()) {
		ui.Column(c).Grow(1).Basis(0).MinWidth(0).Padding(7, 9).Gap(2).Radius(7).Background(p.Subtle).Border(1, p.BorderSubtle).Children(func() {
			ui.Text(c, tr(label)).SingleLine().FontSize(p.font(fontMicro)).TextColor(p.Text3)
			ui.Row(c).Gap(5).Children(body)
		})
	}

	ui.Column(c.Key("telemetry-bento")).Shrink(0).Padding(8, 10, 10, 10).Gap(6).
		Background(p.Chrome.Alpha(0.55)).BorderWidth(1, 0, 0, 0).BorderColor(p.BorderSubtle).Children(func() {
		ui.Row(c).Gap(6).Children(func() {
			cell("输入 / 缓存命中", func() {
				ui.Text(c, inText).Font(monoFont).FontSize(p.font(fontBody)).FontWeight(650).TextColor(p.Text).NoWrap()
				if cacheText != "" {
					ui.Text(c, cacheText).Font(monoFont).FontSize(p.font(fontMicro)).FontWeight(600).TextColor(p.Success).NoWrap()
				}
			})
			cell("输出吞吐 TPS", func() {
				ui.Text(c, tpsText).Font(monoFont).FontSize(p.font(fontBody)).FontWeight(650).TextColor(p.Text).NoWrap()
				if tpsUnit != "" {
					ui.Text(c, tpsUnit).Font(monoFont).FontSize(p.font(fontMicro)).TextColor(p.Text3).NoWrap()
				}
			})
		})
		ui.Row(c).Gap(6).Children(func() {
			cell("最近回合耗时", func() {
				ui.Text(c, turnText).Font(monoFont).FontSize(p.font(fontBody)).FontWeight(650).TextColor(p.Text).NoWrap()
			})
			cell("工作区改动", func() {
				if len(v.Changes) > 0 {
					d.diffStat(c, p, add, del)
					ui.Text(c, fmt.Sprintf("(%d)", len(v.Changes))).Font(monoFont).FontSize(p.font(fontMicro)).TextColor(p.Text3).NoWrap()
				} else {
					ui.Text(c, tr("无改动")).Font(monoFont).FontSize(p.font(fontSmall)).FontWeight(550).TextColor(p.Text3).NoWrap()
				}
			})
		})
	})
}

// workTabs is a segmented control of the work panel's pages, with a
// raised thumb that springs to the chosen one and counts beside them.
func (d *desktop) workTabs(c *ui.Context, p palette) {
	v := d.current()
	track := ui.Row(c).Shrink(1).MinWidth(0).Wrap().Gap(1).Padding(2).Radius(7).Background(p.Subtle).Border(1, p.BorderSubtle).Role(ui.RoleTabList).Label(tr("工作区"))
	state := ui.Local(track, "tabs", func() []ui.Rect {
		return make([]ui.Rect, len(workTabs))
	})
	if d.tab >= 0 && d.tab < len(*state) && (*state)[d.tab].W == 0 {
		c.After(16 * time.Millisecond)
	}
	if d.tab >= 0 && d.tab < len(*state) && (*state)[d.tab].W > 0 {
		selected := (*state)[d.tab]
		spring := ui.Animation{Spring: ui.SpringOptions{Duration: 340 * time.Millisecond, Bounce: 0.14}}
		x, y := track.AnimateTo("x", selected.X, spring), track.AnimateTo("y", selected.Y, spring)
		w, h := track.AnimateTo("w", selected.W, spring), track.AnimateTo("h", selected.H, spring)
		track.Draw(func(painter *ui.Painter, r ui.Rect) {
			thumb := ui.Rect{X: r.X + x, Y: r.Y + y, W: w, H: h}
			painter.Shadow(thumb, 5, 0, 1, 2, 0, ui.RGBA(0, 0, 0, 0.12))
			painter.Fill(thumb, p.Thumb, 5)
		})
	}
	track.Children(func() {
		origin := track.Bounds()
		termCount := 0
		for _, item := range v.Items {
			if item.Kind == "tool" && (item.Name == "bash" || item.Name == "powershell") {
				termCount++
			}
		}
		for i, tab := range workTabs {
			count := ""
			if i == 0 && len(v.Changes) > 0 {
				count = fmtInt(len(v.Changes))
			} else if i == 2 && termCount > 0 {
				count = fmtInt(termCount)
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
			b := ui.ButtonBase(c).Label(tr(tab.Label)).Role(ui.RoleTab).Height(26).Padding(0, 6).Gap(4).Shrink(0).Radius(5).
				FontSize(p.font(fontCaption)).TextColor(p.Text3).Cursor(ui.CursorDefault)
			if current {
				b.TextColor(p.Text).FontWeight(550)
				if (*state)[i].W == 0 {
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
					ui.Text(c, count).Font(monoFont).FontSize(p.font(fontMicro)).TextColor(p.Text3)
				}
			})
			if r := b.Bounds(); r.W > 0 {
				(*state)[i] = ui.Rect{X: r.X - origin.X, Y: r.Y - origin.Y, W: r.W, H: r.H}
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
	ui.Text(c, tr(text)).Padding(20).Margin(6).FontSize(p.font(fontSmall)).LineHeight(1.6).TextColor(p.Text3).TextAlign(ui.Center).Border(1, p.Border).BorderStyle(ui.BorderDashed).Radius(10)
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
		text := ui.Text(c, task.Title).Grow(1).Basis(0).MinWidth(0).SingleLine().FontSize(p.font(fontSmall))
		switch task.Status {
		case "done":
			text.Strikethrough().TextColor(p.Text3)
		case "running":
			text.FontWeight(600)
		}
		label := map[string]string{"done": tr("完成"), "running": tr("进行中"), "failed": tr("失败"), "pending": tr("待执行")}[task.Status]
		muted(c, p, label).FontSize(p.font(fontCaption))
	})
}

func (d *desktop) diffStat(c *ui.Context, p palette, a, r int) {
	if a == 0 && r == 0 {
		return
	}
	ui.Row(c).Gap(4).Children(func() {
		ui.Text(c, fmt.Sprintf("+%d", a)).Font(monoFont).FontSize(p.font(fontMicro)).TextColor(p.Success)
		ui.Text(c, fmt.Sprintf("−%d", r)).Font(monoFont).FontSize(p.font(fontMicro)).TextColor(p.Danger)
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
			ui.Text(c, entry.Name).Grow(1).SingleLine().FontSize(p.font(fontSmall))
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
	}).Grow(1).Basis(0).MinHeight(0).Padding(0, 8, 8, 8).Role(ui.RoleTree).Label(tr("工作区文件"))
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
			ui.Text(c, basename(preview.Path)).FontSize(p.font(fontSmall)).FontWeight(600).SingleLine()
			muted(c, p, preview.Path).FontSize(p.font(fontCaption)).SingleLine()
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
				ui.Text(c, fmtInt(i+1)).Width(42).Padding(0, 10, 0, 0).TextAlign(ui.End).Font(monoFont).FontSize(p.font(fontCaption)).TextColor(p.Text4)
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

func formatUSD(usd float64) string {
	if usd == 0 {
		return "$0.00"
	}
	if usd >= 0.01 {
		return fmt.Sprintf("$%.4f", usd)
	}
	return fmt.Sprintf("$%.6f", usd)
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
	ui.Row(c).Margin(10, 0, 0, 0).Padding(2, 6).Children(func() {
		muted(c, p, tr("会话统计")).Grow(1).FontSize(p.font(fontCaption))
		muted(c, p, fmt.Sprintf(tr("%d 次请求"), requests)).FontSize(p.font(fontCaption))
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
	values := []struct{ Label, Value string }{{tr("输入 token"), "—"}, {tr("输出 token"), "—"}, {tr("缓存命中率"), "—"}, {tr("平均生成 TPS"), "—"}}
	if known {
		values[0].Value = number(usage.Input)
		values[1].Value = number(usage.Output)
		if usage.Input > 0 {
			values[2].Value = fmt.Sprintf("%.1f%%", cacheRatio*100)
		}
	}
	if stats != nil && stats.TPS != nil {
		values[3].Value = fmt.Sprintf("%.1f tok/s", *stats.TPS)
	}
	calls := 0
	if stats != nil {
		calls = stats.ToolCalls
	}
	card := ui.Column(c).FillWidth().Padding(12, 14).Gap(10).Radius(10).Border(1, p.BorderSubtle).Background(p.Elevated)
	if !p.Dark {
		card.Shadow(0, 1, 2, 0, ui.RGBA(0, 0, 0, 0.035))
	}
	card.Children(func() {
		if known && usage.Input+usage.Output > 0 {
			ui.Row(c).Height(4).Radius(99).Clip().Background(p.Active).Label(tr("输入与输出 token 分布")).Children(func() {
				ui.Box(c).WidthPercent(inputRatio * 100).FillHeight().Background(p.Ink)
				ui.Box(c).Grow(1).FillHeight().Background(p.Accent)
			})
		}
		for pair := 0; pair < 2; pair++ {
			ui.Row(c).Gap(12).Children(func() {
				for _, value := range values[pair*2 : pair*2+2] {
					ui.Column(c).Grow(1).Basis(0).Gap(2).Children(func() {
						muted(c, p, value.Label).FontSize(p.font(fontCaption))
						ui.Text(c, value.Value).Font(monoFont).FontSize(p.font(fontProse)).FontWeight(600).TextColor(p.Text)
					})
				}
			})
		}
		ui.Box(c).FillWidth().Height(1).Background(p.BorderSubtle)
		ui.Row(c).Gap(8).Children(func() {
			muted(c, p, fmt.Sprintf(tr("轮次 %d"), state.Rounds)+" · "+fmt.Sprintf(tr("工具调用 %d"), calls)).Grow(1).FontSize(p.font(fontCaption))
			if stats != nil && stats.Cost != nil {
				muted(c, p, tr("估算费用 ")+formatUSD(*stats.Cost)).FontSize(p.font(fontCaption))
			} else if known && (usage.CacheRead > 0 || usage.CacheWrite > 0) {
				muted(c, p, tr("缓存读取 ")+number(usage.CacheRead)).FontSize(p.font(fontCaption))
			}
		})
	})
	if stats != nil && len(stats.Samples) > 0 {
		samples := stats.Samples[max(0, len(stats.Samples)-32):]
		open := v.Open["usage-details"]
		toggle := ui.ButtonBase(c).Label(trf("详细统计（最近 %d 次）", len(samples))).Expanded(open).FillWidth().Height(28).Padding(0, 8).Gap(6).Radius(9).Justify(ui.Start).Transition(fastMotion)
		if toggle.Hovered() {
			toggle.Background(p.Hover)
		}
		toggle.Children(func() {
			ui.Text(c, trf("详细统计（最近 %d 次）", len(samples))).Grow(1).FontSize(p.font(fontCaption)).TextColor(p.Text3)
			disclosure(c, p, open)
		})
		if toggle.Clicked() {
			v.Open["usage-details"] = !open
		}
		if open {
			maxTokens, maxTPS := 1, 1.0
			for _, sample := range samples {
				maxTokens = max(maxTokens, sample.Input+sample.Output)
				if sample.TPS != nil {
					maxTPS = max(maxTPS, *sample.TPS)
				}
			}
			muted(c, p, tr("每轮 token")).Padding(2, 6).FontSize(p.font(fontCaption))
			ui.Row(c).Height(104).Gap(3).Padding(4, 6).AlignItems(ui.End).Children(func() {
				for _, sample := range samples {
					ui.Column(c).Grow(1).Basis(0).Height(96).Justify(ui.End).Gap(2).Children(func() {
						bar := ui.Column(c).Grow(1).Justify(ui.End).Tooltip(fmt.Sprintf(tr("#%d · 输入 %d · 缓存 %d · 输出 %d"), sample.Round, sample.Input, sample.Cached, sample.Output))
						bar.Children(func() {
							ui.Box(c).Height(76 * float32(sample.Output) / float32(maxTokens)).Background(p.Info)
							ui.Box(c).Height(76 * float32(min(sample.Input, sample.Cached)) / float32(maxTokens)).Background(p.Success)
							ui.Box(c).Height(76 * float32(max(0, sample.Input-sample.Cached)) / float32(maxTokens)).Background(p.Accent)
						})
						ui.Text(c, fmtInt(sample.Round)).TextAlign(ui.Center).FontSize(p.font(fontMicro)).TextColor(p.Text4)
					})
				}
			})
			muted(c, p, tr("输出速度")).Padding(2, 6).FontSize(p.font(fontCaption))
			ui.Box(c).Height(64).Margin(0, 6).Draw(func(painter *ui.Painter, r ui.Rect) {
				var path ui.Path
				started := false
				for i, sample := range samples {
					if sample.TPS == nil {
						continue
					}
					x := r.X + 4 + float32(i)*(r.W-8)/float32(max(1, len(samples)-1))
					y := r.Y + 56 - float32(*sample.TPS/maxTPS)*48
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
			if stats.RequestTPS != nil {
				muted(c, p, fmt.Sprintf(tr("请求整体速度（含首字等待）：%.1f tok/s"), *stats.RequestTPS)).Padding(2, 6).FontSize(p.font(fontCaption))
			}
			for _, sample := range samples {
				speed := "—"
				if sample.TPS != nil {
					speed = fmt.Sprintf("%.1f", *sample.TPS)
				}
				ui.Text(c, fmt.Sprintf(tr("#%d  输入 %d  输出 %d  缓存 %d  TPS %s"), sample.Round, sample.Input, sample.Output, sample.Cached, speed)).FontSize(p.font(fontCaption)).Padding(3, 6).Selectable()
				if sample.FirstTokenSeconds != nil && sample.GenerationSeconds != nil {
					muted(c, p, fmt.Sprintf(tr("首字等待 %.2fs · 生成 %.2fs"), *sample.FirstTokenSeconds, *sample.GenerationSeconds)).FontSize(p.font(fontMicro)).Padding(0, 6, 3, 6)
				}
			}
		}
	}
}

var _ = content.DiffLine{}
