package main

import (
	"strings"

	"github.com/egoist/mygo/ui"
)

type instructionsDraft struct {
	Path    string `json:"path"`
	Content string `json:"content"`
	Exists  bool   `json:"exists"`
	Saved   string
	Loading bool
	Saving  bool
	Error   string
	Status  string
}

func (d *desktop) loadInstructions(s *settingsState) {
	draft := &instructionsDraft{Loading: true}
	s.Instructions = draft
	var result instructionsDraft
	d.request(d.current(), "getInstructions", nil, &result, func(err error) {
		draft.Loading = false
		if err != nil {
			draft.Error = err.Error()
			return
		}
		draft.Path, draft.Content, draft.Exists, draft.Saved = result.Path, result.Content, result.Exists, result.Content
	})
}

func (d *desktop) instructionsSettings(c *ui.Context, p palette, s *settingsState) {
	if s.Instructions == nil {
		d.loadInstructions(s)
	}
	a := s.Instructions
	section(c, p, "全局 AGENTS.md", "这些指令适用于所有工作区。项目 AGENTS.md 在对应目录内优先；保存后从下一次模型请求生效。", func() {
		if a.Loading {
			muted(c, p, tr("正在读取指令…"))
			return
		}
		if a.Path != "" {
			ui.Text(c, a.Path).FontSize(p.font(10.92)).TextColor(p.Text3).Selectable()
		}
		ui.Text(c, tr("指令内容")).FontSize(p.font(11.48)).FontWeight(550)
		ui.TextArea(c, &a.Content).Label(tr("全局指令内容")).FillWidth().Height(340).FontSize(p.font(11.76)).Disabled(a.Saving)
		ui.Row(c).Gap(12).Children(func() {
			muted(c, p, trf("%d / 64000 字节", len([]byte(a.Content)))).Grow(1)
			if a.Content != a.Saved {
				badge(c, p, tr("未保存"), p.Warning, p.WarningSoft)
			}
		})
		if a.Error != "" {
			ui.Text(c, tr(a.Error)).TextColor(p.Danger).FontSize(p.font(11.48))
		}
		if a.Status != "" {
			muted(c, p, tr(a.Status))
		}
		ui.Row(c).Wrap().Gap(8).Children(func() {
			label := "保存指令"
			if a.Saving {
				label = "正在保存…"
			}
			if button(c, p, label, "", true).Disabled(a.Saving || len([]byte(a.Content)) > 64000 || a.Content == a.Saved).Clicked() {
				a.Saving, a.Error, a.Status = true, "", ""
				content := a.Content
				var result instructionsDraft
				d.request(d.current(), "saveInstructions", map[string]any{"content": content}, &result, func(err error) {
					a.Saving = false
					if err != nil {
						a.Error = err.Error()
						return
					}
					a.Saved, a.Exists = content, result.Exists
					a.Status = "已保存，下次模型请求生效。"
				})
			}
			if button(c, p, "还原已保存内容", "", false).Disabled(a.Saving || a.Content == a.Saved).Clicked() {
				a.Content, a.Status, a.Error = a.Saved, "", ""
			}
			if button(c, p, "重新读取文件", "RefreshCw", false).Disabled(a.Saving || a.Content != a.Saved).Clicked() {
				d.loadInstructions(s)
			}
		})
		if strings.TrimSpace(a.Content) == "" {
			muted(c, p, tr("留空并保存即可停用全局指令。"))
		}
	})
}
