package main

import (
	"strings"

	"github.com/egoist/mygo/ui"
	"minicode.desktop/internal/model"
)

type localExtension struct {
	ID          string `json:"id"`
	Kind        string `json:"kind"`
	Name        string `json:"name"`
	Description string `json:"description"`
	Scope       string `json:"scope"`
	Source      string `json:"source"`
	Path        string `json:"path"`
	Status      string `json:"status"`
	Detail      string `json:"detail"`
	Enabled     bool   `json:"enabled"`
	Supported   bool   `json:"supported"`
	ParentID    string `json:"parentId"`
	RuntimeName string `json:"runtimeName"`
}

type localExtensions struct {
	Entries     []localExtension `json:"entries"`
	Diagnostics []string         `json:"diagnostics"`
}

type extensionsSettingsState struct {
	Catalog localExtensions
	Query   string
	Scope   string
	Loading bool
	Pending string
	Error   string
}

func (d *desktop) loadLocalExtensions(s *settingsState) {
	if s.Extensions == nil {
		s.Extensions = &extensionsSettingsState{Scope: "all"}
	}
	e := s.Extensions
	e.Loading, e.Error = true, ""
	var result localExtensions
	d.request(d.current(), "getLocalExtensions", nil, &result, func(err error) {
		e.Loading = false
		if err != nil {
			e.Error = err.Error()
			return
		}
		e.Catalog = result
	})
}

func visibleExtensions(e *extensionsSettingsState, kind string) []localExtension {
	result := []localExtension{}
	query := strings.ToLower(strings.TrimSpace(e.Query))
	for _, item := range e.Catalog.Entries {
		if item.Kind != kind || item.ParentID != "" || e.Scope != "all" && item.Scope != e.Scope {
			continue
		}
		if query != "" && !strings.Contains(strings.ToLower(item.Name+" "+item.Description+" "+item.Source+" "+item.Path), query) {
			continue
		}
		result = append(result, item)
	}
	return result
}

func (d *desktop) extensionSettings(c *ui.Context, p palette, s *settingsState, kind string) {
	if s.Extensions == nil {
		d.loadLocalExtensions(s)
	}
	e := s.Extensions
	section(c, p, "本地资源", "扫描本机已有配置，按需启用。开关仅保存在 MiniCode，下一回合生效；原应用配置不受影响。", func() {
		ui.Row(c).Wrap().Gap(10).Children(func() {
			segmented(c, p, "资源范围", e.Scope, [][2]string{{"all", "全部"}, {"global", "全局"}, {"project", "项目"}}, func(value string) { e.Scope = value })
			ui.TextInput(c, &e.Query).Label(tr("搜索本地资源")).Placeholder(tr("搜索名称或来源")).MinWidth(180).Grow(1).Height(32)
			if button(c, p, "扫描本地配置", "RefreshCw", false).Disabled(e.Loading || e.Pending != "").Clicked() {
				d.loadLocalExtensions(s)
			}
		})
		if e.Error != "" {
			ui.Text(c, tr(e.Error)).TextColor(p.Danger).FontSize(p.font(11.48))
		}
		if e.Loading {
			ui.Row(c).Gap(8).Children(func() { spinner(c, p); muted(c, p, tr("正在扫描本地配置…")) })
			return
		}
		entries := visibleExtensions(e, kind)
		muted(c, p, trf("%d 个资源", len(entries))).FontSize(p.font(10.92))
		if len(entries) == 0 {
			empty(c, p, "未发现匹配的本地资源。")
		}
		for _, item := range entries {
			ui.Row(c).Key(item.ID).FillWidth().Padding(14).Gap(14).Radius(10).Border(1, p.BorderSubtle).Background(p.Panel).Children(func() {
				ui.Column(c).Grow(1).Basis(0).MinWidth(0).Gap(5).Children(func() {
					ui.Row(c).Wrap().Gap(6).Children(func() {
						ui.Text(c, item.Name).FontSize(p.font(12.32)).FontWeight(600)
						label := "全局"
						if item.Scope == "project" {
							label = "项目"
						}
						badge(c, p, label, p.Text3, p.Subtle).NoWrap().Shrink(0)
						badge(c, p, item.Source, p.Text3, p.Subtle).NoWrap().Shrink(0)
						if !item.Supported {
							badge(c, p, "暂不支持", p.Warning, p.WarningSoft).NoWrap().Shrink(0)
						}
					})
					if item.Description != "" {
						ui.Text(c, item.Description).FontSize(p.font(11.48)).TextColor(p.Text3).MaxLines(2).Tooltip(item.Description)
					}
					ui.Text(c, item.Path).FontSize(p.font(10.36)).TextColor(p.Text4).SingleLine().Tooltip(item.Path)
					if item.Detail != "" {
						muted(c, p, item.Detail).MaxLines(2).Tooltip(item.Detail)
					}
					if !item.Supported && item.Status != "" {
						muted(c, p, tr(item.Status)).MaxLines(2)
					}
				})
				on := item.Enabled
				if e.Pending == item.ID {
					spinner(c, p)
				}
				if toggle(c, p, &on, trf("启用本地资源 %s", item.Name)).Disabled(!item.Supported || e.Pending != "").Changed() {
					e.Pending, e.Error = item.ID, ""
					v := d.current()
					var result localExtensions
					d.request(v, "setLocalExtensionEnabled", map[string]any{"id": item.ID, "enabled": on}, &result, func(err error) {
						e.Pending = ""
						if err != nil {
							e.Error = err.Error()
							return
						}
						e.Catalog = result
						d.refresh(v, true)
						d.refreshState(v)
					})
				}
			})
		}
		for _, diagnostic := range e.Catalog.Diagnostics {
			muted(c, p, diagnostic)
		}
	})
	switch kind {
	case "skill":
		section(c, p, "当前会话已加载", "启用的技能按需读取正文；这里列出已加入当前会话的技能。", func() {
			count := 0
			for _, skill := range d.current().Capabilities.Skills {
				if !skill.Active {
					continue
				}
				count++
				settingsRow(c, p, skill.Name, skill.Description, func() {
					if button(c, p, "卸载技能", "", false).Clicked() {
						d.capabilityAction(s, "setSkillActive", map[string]any{"name": skill.Name, "active": false})
					}
				})
			}
			if count == 0 {
				empty(c, p, "当前会话还没有加载技能正文。")
			}
		})
	case "mcp":
		section(c, p, "连接状态", "启用的 MCP 服务在运行回合中连接，也可以手动测试连接。", func() {
			if button(c, p, "测试已启用服务", "RefreshCw", false).Disabled(s.Working || e.Pending != "").Clicked() {
				s.Working = true
				var result model.Discovery
				d.request(d.current(), "refreshMcp", nil, &result, func(err error) {
					s.Working = false
					if err != nil {
						s.Error = err.Error()
					} else {
						d.discovery = result
					}
				})
			}
			for _, status := range d.discovery.Servers {
				name := status.Server
				for _, item := range e.Catalog.Entries {
					if item.Kind == "mcp" && item.RuntimeName == status.Server {
						name = item.Name
						break
					}
				}
				label := "已连接"
				if status.Error != nil {
					label = strings.ReplaceAll(*status.Error, status.Server, name)
				}
				settingsRow(c, p, name, label, func() {})
			}
		})
	case "plugin":
		if len(d.current().Capabilities.Plugins) == 0 {
			return
		}
		section(c, p, "MiniCode 项目插件", "项目插件沿用现有清单与内容锁定。", func() {
			if button(c, p, "更新锁定", "RefreshCw", false).Clicked() {
				d.capabilityAction(s, "lockPlugins", nil)
			}
			for _, plugin := range d.current().Capabilities.Plugins {
				settingsRow(c, p, plugin.Name+" · v"+plugin.Version, plugin.Path, func() {
					on := plugin.Enabled
					if toggle(c, p, &on, trf("启用 %s", plugin.Name)).Changed() {
						d.capabilityAction(s, "setPluginEnabled", map[string]any{"name": plugin.Name, "enabled": on})
					}
				})
			}
		})
	}
}
