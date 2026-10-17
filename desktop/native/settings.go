package main

import (
	"encoding/json"
	"fmt"
	"slices"
	"strconv"
	"strings"

	"github.com/egoist/mygo/ui"
	"minicode.desktop/internal/model"
)

type settingsState struct {
	Tab                                   string
	Error, Status, TraceQuery, Acceptance string
	Budget                                [3]string
	Working                               bool
	Service                               *model.Service
	Found                                 []model.ServiceModel
	ModelQuery, CustomModel               string
	OpenModels                            map[int]bool
	DeleteService                         string
	Agent                                 *model.Agent
	DeleteAgent                           string
	Instructions                          *instructionsDraft
	Extensions                            *extensionsSettingsState
}

var settingsTabs = []struct{ ID, Icon, Label string }{
	{"general", "Palette", "通用"}, {"model", "SlidersHorizontal", "模型与预算"},
	{"permissions", "Shield", "权限"}, {"instructions", "FileText", "指令"}, {"skills", "Sparkles", "技能"}, {"mcp", "Cable", "MCP 服务"},
	{"plugins", "Blocks", "插件"}, {"subagents", "Bot", "子助手"}, {"shortcuts", "Keyboard", "快捷键"},
	{"inspector", "Activity", "运行记录"}, {"about", "Info", "关于"},
}

func (d *desktop) openSettings(tab string) {
	v := d.current()
	d.modal = ""
	d.settings = &settingsState{Tab: tab, Acceptance: v.State.Acceptance,
		Budget: [3]string{fmtInt(v.State.Budget.MaxRounds), fmtInt(v.State.Budget.MaxTotalTokens), strconv.FormatFloat(v.State.Budget.MaxSeconds, 'f', -1, 64)}, OpenModels: map[int]bool{}}
	d.loadConfiguration()
}

func section(c *ui.Context, p palette, title, description string, fn func()) {
	ui.Column(c).Padding(18, 0).BorderWidth(0, 0, 1, 0).BorderColor(p.BorderSubtle).Gap(12).Children(func() {
		ui.Column(c).Gap(3).Children(func() {
			ui.Text(c, tr(title)).FontSize(p.font(12.88)).FontWeight(650)
			if description != "" {
				muted(c, p, description).FontSize(p.font(11.48))
			}
		})
		fn()
	})
}

func settingsRow(c *ui.Context, p palette, label, description string, fn func()) {
	ui.Row(c).MinHeight(48).Padding(6, 0).Gap(16).Children(func() {
		ui.Column(c).Grow(1).Basis(0).MinWidth(0).Gap(1).Children(func() {
			ui.Text(c, tr(label)).FontSize(p.font(12.32)).FontWeight(550)
			if description != "" {
				muted(c, p, description).FontSize(p.font(10.92))
			}
		})
		fn()
	})
}

func field(c *ui.Context, p palette, label string, value *string, password bool) ui.Element {
	var e ui.Element
	ui.Column(c).Gap(5).Children(func() {
		ui.Text(c, tr(label)).FontSize(p.font(11.48)).TextColor(p.Text2)
		e = ui.TextInput(c, value).Label(tr(label)).FillWidth().Height(32).FontSize(p.font(11.76))
		if password {
			e.Password()
		}
	})
	return e
}

func numericField(c *ui.Context, p palette, label string, value *int, minimum int) {
	root := ui.Column(c).FillWidth().Gap(5)
	text := ui.Local(root, "number", func() string { return strconv.Itoa(*value) })
	root.Children(func() {
		ui.Text(c, tr(label)).FontSize(p.font(11.48)).TextColor(p.Text2)
		input := ui.TextInput(c, text).Label(tr(label)).Height(32).FillWidth().FontSize(p.font(11.76))
		if input.Changed() {
			if n, err := strconv.Atoi(*text); err == nil && n >= minimum && n <= 2147483647 {
				*value = n
			}
		}
		if !input.Focused() {
			*text = strconv.Itoa(*value)
		}
		if n, err := strconv.Atoi(*text); err != nil || n < minimum || n > 2147483647 {
			ui.Text(c, fmt.Sprintf(tr("请输入 %d 至 2147483647 的整数"), minimum)).FontSize(p.font(10.36)).TextColor(p.Danger)
		}
	})
}

func segmented(c *ui.Context, p palette, label, value string, options [][2]string, changed func(string)) {
	selected := value
	ui.Row(c).Gap(2).Padding(3).Radius(8).Background(p.Subtle).Role(ui.RoleRadioGroup).FocusGroup(ui.Horizontal).Label(tr(label)).Children(func() {
		for _, option := range options {
			e := styleButton(ui.RadioBase(c, &selected, option[0]).Label(tr(option[1])), p, false).Height(26).Padding(0, 10).Border(0, ui.Transparent).FontSize(p.font(11.2))
			e.Children(func() { ui.Text(c, tr(option[1])).NoWrap() })
			if option[0] == value {
				e.Background(p.Elevated).TextColor(p.Text)
			}
			if e.Changed() {
				changed(selected)
			}
		}
	})
}

func (d *desktop) settingsView(c *ui.Context, p palette) {
	s := d.settings
	open := s != nil
	if !open {
		return
	}
	ui.DialogBase(c, &open, func(back, panel ui.Element) {
		back.Material(modalBackdrop{p.Overlay})
		_, height := c.Size()
		panel.Width(920).MaxWidthPercent(95).Height(min(680, height-64)).Padding(0).Radius(16).Clip().Background(p.Main).Border(1, p.Border).Shadow(0, 24, 64, 0, ui.RGBA(0, 0, 0, 0.3)).Label(tr("设置")).Row()
		ui.Column(c).Width(212).FillHeight().Padding(16, 10).Gap(1).BorderWidth(0, 1, 0, 0).BorderColor(p.BorderSubtle).Background(p.Panel).Children(func() {
			ui.Text(c, tr("设置")).Padding(2, 10, 12, 10).FontSize(p.font(14.7)).FontWeight(650)
			for _, tab := range settingsTabs {
				b := button(c, p, tab.Label, tab.Icon, false).FillWidth().Height(32).Justify(ui.Start).Padding(0, 10).Gap(9).Border(0, ui.Transparent).FontSize(p.font(12.04)).FontWeight(400)
				if s.Tab == tab.ID {
					b.Background(p.Active).TextColor(p.Text).FontWeight(550)
				}
				if b.Clicked() {
					s.Tab = tab.ID
					s.Error = ""
					s.Service = nil
					s.Agent = nil
				}
			}
		})
		ui.Column(c).Grow(1).Basis(0).FillHeight().Children(func() {
			label := ""
			for _, tab := range settingsTabs {
				if tab.ID == s.Tab {
					label = tab.Label
				}
			}
			ui.Row(c).Height(56).Shrink(0).Padding(0, 12, 0, 28).Children(func() {
				ui.Text(c, tr(label)).FontSize(p.font(14.7)).FontWeight(650).Grow(1)
				if iconButton(c, p, "X", tr("关闭设置")).Clicked() {
					open = false
				}
			})
			ui.Scroll(c.Key(fmt.Sprintf("%s-%t", s.Tab, s.Service != nil))).Grow(1).Basis(0).MinHeight(0).Padding(0, 28, 28, 28).Children(func() {
				if s.Error != "" {
					ui.Text(c, localizeNotice(s.Error)).Margin(0, 0, 10, 0).Padding(8, 12).Radius(8).Background(p.DangerSoft).FontSize(p.font(11.48))
				}
				if d.current().Busy {
					ui.Text(c, tr("模型与思考强度在下一次请求生效，预算在下一回合生效。")).Margin(0, 0, 10, 0).Padding(8, 12).Radius(8).Background(p.WarningSoft).FontSize(p.font(11.48))
				}
				d.settingsContent(c, p, s)
			})
			if s.Tab == "model" && s.Service != nil {
				ui.Row(c).Shrink(0).Padding(12, 28).Gap(8).BorderWidth(1, 0, 0, 0).BorderColor(p.BorderSubtle).Children(func() {
					if button(c, p, tr("保存服务"), "", true).Disabled(s.Working).Clicked() {
						d.configAction(s, "saveService", map[string]any{"service": *s.Service}, func() { s.Service = nil })
					}
					if button(c, p, tr("取消"), "", false).Disabled(s.Working).Clicked() {
						s.Service = nil
					}
				})
			}
		})
	})
	if !open {
		d.settings = nil
	}
}

func (d *desktop) settingsContent(c *ui.Context, p palette, s *settingsState) {
	v := d.current()
	switch s.Tab {
	case "general":
		section(c, p, tr("语言"), "", func() {
			settingsRow(c, p, tr("界面语言"), tr("仅切换界面语言，不影响模型回复或自定义指令。"), func() {
				segmented(c, p, tr("界面语言"), d.preferences.Language, [][2]string{{"zh", "中文"}, {"en", "English"}}, func(value string) {
					d.preferences.Language = normalizeLanguage(value)
					setUILanguage(d.preferences.Language)
					d.persist()
				})
			})
		})
		section(c, p, tr("外观"), "", func() {
			settingsRow(c, p, tr("主题"), tr("跟随系统时会随 Windows 深浅色切换"), func() {
				segmented(c, p, tr("主题"), d.preferences.Theme, [][2]string{{"system", tr("跟随系统")}, {"light", tr("浅色")}, {"dark", tr("深色")}}, func(value string) { d.preferences.Theme = value; d.applyAppearance(); d.persist() })
			})
			settingsRow(c, p, tr("界面缩放"), tr("调整对话与面板的文字大小"), func() {
				segmented(c, p, tr("界面缩放"), fmt.Sprintf("%.1f", d.preferences.FontScale), [][2]string{{"0.9", tr("紧凑")}, {"1.0", tr("标准")}, {"1.1", tr("宽松")}}, func(value string) {
					n, _ := strconv.ParseFloat(value, 32)
					d.preferences.FontScale = float32(n)
					d.persist()
				})
			})
		})
		section(c, p, tr("输入与对话"), "", func() {
			settingsRow(c, p, tr("发送方式"), tr("Enter 发送，Shift + Enter 换行"), func() {
				segmented(c, p, tr("发送方式"), d.preferences.SendKey, [][2]string{{"enter", "Enter"}, {"mod-enter", "Ctrl + Enter"}}, func(value string) { d.preferences.SendKey = value; d.persist() })
			})
			settingsRow(c, p, tr("默认展开工具详情"), tr("在对话中直接显示每一步的参数与输出"), func() {
				if toggle(c, p, &d.preferences.ExpandTools, tr("默认展开工具详情")).Changed() {
					d.persist()
				}
			})
		})
		section(c, p, tr("通知"), "", func() {
			settingsRow(c, p, tr("系统通知"), tr("窗口不在前台时，任务完成或需要审批会发出桌面通知"), func() {
				if toggle(c, p, &d.preferences.Notifications, tr("系统通知")).Changed() {
					d.persist()
				}
			})
		})
		d.updateSettings(c, p)
	case "model":
		d.modelSettings(c, p, s)
		if s.Service != nil {
			return
		}
		supports := false
		for _, m := range d.models {
			if m.ID == v.State.Model {
				supports = m.SupportsEffort
			}
		}
		effortDescription := tr("控制模型用于分析复杂问题的推理预算。")
		if !supports {
			effortDescription = tr("当前模型不支持调整思考强度。")
		}
		section(c, p, tr("思考强度"), effortDescription, func() {
			effortSlider(c, p, v.State.Effort, supports, func(value string) { d.changeState("setEffort", map[string]any{"effort": value}) })
		})
		section(c, p, tr("运行预算"), tr("限制单次任务的资源上限。0 表示不限制，下一回合生效。"), func() {
			ui.Row(c).Gap(12).Children(func() {
				for i, label := range []string{tr("轮次上限"), tr("Token 上限"), tr("时长（秒）")} {
					ui.Column(c).Grow(1).Basis(0).Children(func() { field(c, p, label, &s.Budget[i], false) })
				}
			})
			ui.Row(c).Gap(8).Children(func() {
				if button(c, p, tr("应用预算"), "", true).Clicked() {
					rounds, e1 := strconv.Atoi(s.Budget[0])
					tokens, e2 := strconv.Atoi(s.Budget[1])
					seconds, e3 := strconv.ParseFloat(s.Budget[2], 64)
					if e1 != nil || e2 != nil || e3 != nil || rounds < 0 || tokens < 0 || seconds < 0 {
						s.Error = tr("预算必须是非负数。")
					} else {
						d.changeState("setBudget", map[string]any{"max_rounds": rounds, "max_total_tokens": tokens, "max_seconds": seconds})
					}
				}
				if button(c, p, tr("还原"), "", false).Clicked() {
					s.Budget = [3]string{fmtInt(v.State.Budget.MaxRounds), fmtInt(v.State.Budget.MaxTotalTokens), strconv.FormatFloat(v.State.Budget.MaxSeconds, 'f', -1, 64)}
				}
			})
		})
		section(c, p, tr("验收标准"), tr("选择 YAML 验收配置，任务完成时自动检查结果。保存后用于下一个新会话。"), func() {
			ui.Row(c).Gap(8).Children(func() {
				ui.Column(c).Grow(1).Children(func() { field(c, p, tr("验收文件路径"), &s.Acceptance, false) })
				if button(c, p, tr("选择文件"), "FolderOpen", false).Clicked() {
					d.chooseAcceptance(s)
				}
			})
			ui.Row(c).Gap(8).Children(func() {
				if button(c, p, tr("应用"), "", true).Clicked() {
					d.changeState("setAcceptance", map[string]any{"path": s.Acceptance})
				}
				if button(c, p, tr("清除"), "", false).Clicked() {
					s.Acceptance = ""
					d.changeState("setAcceptance", map[string]any{"path": ""})
				}
			})
		})
	case "permissions":
		section(c, p, tr("工具权限"), tr("选择文件编辑与终端命令的批准方式。只读工具始终直接执行。"), func() {
			ui.Column(c).FillWidth().Gap(8).Role(ui.RoleRadioGroup).FocusGroup(ui.Vertical).Label(tr("工具权限")).Children(func() {
				for i, option := range permissionOptions {
					if permissionChoice(c, p, v.State.PermissionMode, i, false).Changed() {
						d.changeState("setPermissionMode", map[string]any{"mode": option.Value})
					}
				}
			})
		})
		section(c, p, tr("本会话始终允许"), tr("在审批卡片中选择“本会话始终允许”的工具，会在当前会话内自动批准。"), func() {
			if len(v.State.AlwaysAllow) == 0 {
				empty(c, p, tr("当前会话没有自动批准的工具。"))
			} else {
				ui.Row(c).Wrap().Gap(6).Children(func() {
					for _, tool := range v.State.AlwaysAllow {
						badge(c, p, tool, p.Text2, p.Subtle)
					}
				})
				if button(c, p, tr("全部撤销"), "", false).Clicked() {
					d.changeState("clearAlwaysAllow", nil)
				}
			}
		})
	case "instructions":
		d.instructionsSettings(c, p, s)
	case "skills":
		d.extensionSettings(c, p, s, "skill")
	case "mcp":
		d.extensionSettings(c, p, s, "mcp")
	case "plugins":
		d.extensionSettings(c, p, s, "plugin")
	case "subagents":
		d.agentSettings(c, p, s)
	case "shortcuts":
		shortcutList(c, p)
	case "inspector":
		section(c, p, tr("会话运行记录"), fmt.Sprintf(tr("会话 %s · %d 轮 · 上下文 %d tokens"), v.SessionID, v.State.Rounds, v.State.ContextTokens), func() {
			field(c, p, tr("筛选事件"), &s.TraceQuery, false)
			if len(v.Trace) == 0 {
				empty(c, p, tr("开始任务后，这里会记录每一个运行事件。"))
			}
			for i := len(v.Trace) - 1; i >= max(0, len(v.Trace)-150); i-- {
				event := v.Trace[i]
				if !strings.Contains(event.Type, s.TraceQuery) {
					continue
				}
				key := fmt.Sprintf("event-%d", event.Seq)
				ui.Column(c).Margin(4, 0, 0, 0).Radius(8).Border(1, p.BorderSubtle).Background(p.Panel).Children(func() {
					if button(c, p, fmt.Sprintf("#%d  %s  %s", event.Seq, event.Type, model.Time(event.Timestamp).Format("15:04:05")), "ChevronDown", false).FillWidth().Border(0, ui.Transparent).Clicked() {
						v.Open[key] = !v.Open[key]
					}
					if v.Open[key] {
						data, _ := json.MarshalIndent(event.Data, "", "  ")
						logText(c, p, string(data), 260)
					}
				})
			}
		})
	case "about":
		section(c, p, "MiniCode Desktop", tr("本地优先的 Coding Agent 工作台。会话保存在 ~/.minicode/sessions.db。"), func() {
			for _, entry := range [][2]string{{tr("版本"), appVersion}, {"Mygo", "0.2.15"}, {tr("渲染器"), tr("原生 Direct3D 11")}, {tr("平台"), "Windows x64"}} {
				settingsRow(c, p, entry[0], "", func() { ui.Text(c, entry[1]).FontSize(p.font(12.6)) })
			}
			d.checkUpdateButton(c, p)
		})
	}
}

func (d *desktop) capabilityAction(s *settingsState, method string, params map[string]any) {
	v := d.current()
	d.request(v, method, params, nil, func(err error) {
		if err != nil {
			s.Error = err.Error()
		} else {
			d.refresh(v, true)
			d.refreshState(v)
		}
	})
}

func (d *desktop) configAction(s *settingsState, method string, params map[string]any, done func()) {
	v := d.current()
	s.Working = true
	s.Error = ""
	var config model.Configuration
	d.request(v, method, params, &config, func(err error) {
		s.Working = false
		if err != nil {
			s.Error = err.Error()
			return
		}
		d.config = config
		d.refreshConfiguration()
		if done != nil {
			done()
		}
	})
}

func (d *desktop) modelSettings(c *ui.Context, p palette, s *settingsState) {
	v := d.current()
	if s.Service != nil {
		d.serviceEditor(c, p, s)
		return
	}
	d.serviceSettings(c, p, s)
	if len(d.models) == 0 || s.Service != nil {
		return
	}
	section(c, p, tr("当前模型"), tr("切换模型在下一次请求生效；默认模型用于新会话。"), func() {
		options := []selectOption{{Value: "", Label: tr("请选择模型"), Disabled: true}}
		for _, m := range d.models {
			label := m.Provider + " · " + m.Name
			if m.Name == "" {
				label = m.Provider + " · " + m.ID
			}
			options = append(options, selectOption{Value: m.ID, Label: label, Disabled: !m.Available})
		}
		ui.Column(c).FillWidth().Gap(16).Children(func() {
			ui.Column(c).FillWidth().Gap(6).Children(func() {
				muted(c, p, tr("当前会话"))
				selected := v.State.Model
				if selectField(c, p, &selected, options).Label(tr("当前会话模型")).FillWidth().Disabled(len(d.models) == 0).Changed() {
					d.changeState("setModel", map[string]any{"model": selected})
				}
			})
			defaults := append([]selectOption{{Value: "", Label: tr("自动选择可用模型")}}, options[1:]...)
			ui.Column(c).FillWidth().Gap(6).Children(func() {
				muted(c, p, tr("默认模型"))
				value := d.config.DefaultModel
				if selectField(c, p, &value, defaults).Label(tr("默认模型")).FillWidth().Disabled(len(d.models) == 0).Changed() {
					d.configAction(s, "setDefaultModel", map[string]any{"model": value}, nil)
				}
			})
		})
	})
}

func (d *desktop) serviceSettings(c *ui.Context, p palette, s *settingsState) {
	section(c, p, fmt.Sprintf(tr("AI 服务 · %d"), len(d.config.Services)), tr("配置接口与模型，新服务从空白开始。"), func() {
		if len(d.config.Services) == 0 {
			empty(c, p, tr("尚未添加 AI 服务。填写接口地址、密钥和模型 ID，从零配置你的模型。"))
		}
		if button(c, p, tr("添加服务"), "Plus", true).Justify(ui.Start).Clicked() {
			s.Service = &model.Service{APIStyle: "openai", Enabled: true, Models: []model.ServiceModel{}}
			s.Found = nil
			s.Status = ""
			s.ModelQuery, s.CustomModel = "", ""
			s.OpenModels = map[int]bool{}
		}
		for _, service := range d.config.Services {
			settingsRow(c, p, service.Name, service.BaseURL+fmt.Sprintf(tr(" · %d 个模型"), len(service.Models)), func() {
				if iconButton(c, p, "Pencil", tr("编辑 ")+service.Name).Clicked() {
					copy := service
					copy.Models = slices.Clone(service.Models)
					copy.APIKey = ""
					s.Service = &copy
					s.Found = nil
					s.Status = ""
					s.ModelQuery, s.CustomModel = "", ""
					s.OpenModels = map[int]bool{}
				}
				on := service.Enabled
				if toggle(c, p, &on, tr("启用 ")+service.Name).Changed() {
					service.Enabled = on
					d.configAction(s, "saveService", map[string]any{"service": service}, nil)
				}
				if iconButton(c, p, "Trash2", tr("删除 ")+service.Name).Clicked() {
					s.DeleteService = service.ID
				}
			})
			if s.DeleteService == service.ID {
				ui.Row(c).Gap(8).Children(func() {
					muted(c, p, tr("删除此服务配置？"))
					if button(c, p, tr("删除"), "", false).Clicked() {
						d.configAction(s, "deleteService", map[string]any{"id": service.ID}, func() { s.DeleteService = "" })
					}
					if button(c, p, tr("取消"), "", false).Clicked() {
						s.DeleteService = ""
					}
				})
			}
		}
	})
}

func (d *desktop) serviceEditor(c *ui.Context, p palette, s *settingsState) {
	service := s.Service
	title := tr("添加 AI 服务")
	if service.ID != "" {
		title = tr("编辑 AI 服务")
	}
	section(c, p, title, tr("保存后可随时继续添加或编辑模型。已存密钥留空即可保留。"), func() {
		ui.Row(c).Gap(12).Children(func() {
			ui.Column(c).Grow(1).Basis(0).Children(func() { field(c, p, tr("服务名称"), &service.Name, false) })
			ui.Column(c).Grow(1).Basis(0).Gap(5).Children(func() {
				muted(c, p, tr("接口格式"))
				if selectField(c, p, &service.APIStyle, []selectOption{{Value: "openai", Label: "OpenAI Chat Completions"}, {Value: "anthropic", Label: "Anthropic Messages"}}).Label(tr("接口格式")).FillWidth().Height(32).Changed() && service.APIStyle == "anthropic" {
					for i := range service.Models {
						service.Models[i].InputCapabilities.Audio = false
						service.Models[i].InputCapabilities.Video = false
					}
				}
			})
		})
		field(c, p, tr("接口地址"), &service.BaseURL, false)
		key := field(c, p, tr("API 密钥"), &service.APIKey, true)
		if service.HasAPIKey {
			key.Placeholder(tr("已保存密钥，留空保留"))
		}
		ui.Row(c).Gap(8).Children(func() {
			if button(c, p, tr("测试连接 / 获取列表"), "RefreshCw", false).Disabled(s.Working).Clicked() {
				s.Working = true
				s.Error = ""
				var result struct {
					Models  []model.ServiceModel `json:"models"`
					Message string               `json:"message"`
				}
				d.request(d.current(), "fetchServiceModels", map[string]any{"service": *service}, &result, func(err error) {
					s.Working = false
					if err != nil {
						s.Error = err.Error()
					} else {
						s.Found = result.Models
						s.Status = result.Message
					}
				})
			}
		})
		if s.Status != "" {
			muted(c, p, localizeNotice(s.Status)).FontSize(p.font(11.2))
		}
	})
	section(c, p, tr("添加模型"), tr("获取服务列表或输入模型 ID。"), func() {
		ui.Column(c).FillWidth().Gap(12).Children(func() {
			ui.Row(c).FillWidth().Gap(8).Children(func() {
				ui.TextInput(c, &s.CustomModel).Label(tr("自定义模型 ID")).Placeholder(tr("输入模型 ID")).Grow(1).Basis(0).MinWidth(0).Height(32)
				if button(c, p, tr("添加"), "Plus", false).Disabled(strings.TrimSpace(s.CustomModel) == "").Clicked() {
					id := strings.TrimSpace(s.CustomModel)
					if !slices.ContainsFunc(service.Models, func(m model.ServiceModel) bool { return m.ModelID == id }) {
						service.Models = append(service.Models, model.ServiceModel{ModelID: id, Name: id, ContextWindow: 200000, MaxOutput: 8192})
						s.OpenModels[len(service.Models)-1] = true
					}
					s.CustomModel = ""
				}
			})
			if len(s.Found) > 0 {
				ui.Column(c).FillWidth().Gap(8).Children(func() {
					field(c, p, tr("搜索服务模型"), &s.ModelQuery, false)
					ui.Scroll(c).FillWidth().MaxHeight(180).Children(func() {
						for _, found := range s.Found {
							if !strings.Contains(strings.ToLower(found.ModelID), strings.ToLower(s.ModelQuery)) {
								continue
							}
							selected := slices.ContainsFunc(service.Models, func(m model.ServiceModel) bool { return m.ModelID == found.ModelID })
							if ui.Checkbox(c, &selected, found.ModelID).Changed() {
								if selected {
									service.Models = append(service.Models, found)
								} else {
									service.Models = slices.DeleteFunc(service.Models, func(m model.ServiceModel) bool { return m.ModelID == found.ModelID })
								}
							}
						}
					})
				})
			}
		})
	})
	section(c, p, fmt.Sprintf(tr("已配置模型 · %d"), len(service.Models)), tr("展开模型，编辑名称、上下文窗口与输出上限。"), func() {
		ui.Column(c).FillWidth().Gap(12).Children(func() {
			if len(service.Models) == 0 {
				empty(c, p, tr("尚未添加模型。"))
			}
			remove := -1
			for i := range service.Models {
				m := &service.Models[i]
				ui.Column(c.Key(i)).FillWidth().Border(1, p.Border).Radius(10).Padding(14).Gap(12).Children(func() {
					label := m.Name
					if label == "" {
						label = m.ModelID
					}
					head := ui.ButtonBase(c).Label(tr("编辑模型 ") + m.ModelID).FillWidth().Height(28).Gap(8).Justify(ui.Start)
					head.Children(func() {
						ui.Text(c, label).SingleLine().Grow(1).Basis(0).MinWidth(0).FontSize(p.font(12.04)).FontWeight(600)
						icon(c, map[bool]string{true: "ChevronDown", false: "ChevronRight"}[s.OpenModels[i]], 14, p.Text3)
					})
					if head.Clicked() {
						s.OpenModels[i] = !s.OpenModels[i]
					}
					if s.OpenModels[i] {
						field(c, p, tr("模型 ID"), &m.ModelID, false)
						field(c, p, tr("显示名称"), &m.Name, false)
						ui.Row(c).FillWidth().Gap(16).Children(func() {
							ui.Column(c).Grow(1).Basis(0).MinWidth(0).Children(func() { numericField(c, p, tr("上下文窗口"), &m.ContextWindow, 2) })
							ui.Column(c).Grow(1).Basis(0).MinWidth(0).Children(func() { numericField(c, p, tr("输出上限"), &m.MaxOutput, 1) })
						})
						ui.Checkbox(c, &m.SupportsEffort, tr("支持思考强度")).Disabled(service.APIStyle != "openai").FontSize(p.font(11.48))
						muted(c, p, tr("仅适用于支持 reasoning_effort 的 OpenAI 兼容模型。")).FontSize(p.font(10.36))
						ui.Column(c).FillWidth().Gap(6).Children(func() {
							muted(c, p, tr("输入能力"))
							ui.Row(c).Wrap().Gap(16).Children(func() {
								for _, option := range []struct {
									Label  string
									Value  *bool
									OpenAI bool
								}{{tr("图片"), &m.InputCapabilities.Images, false}, {"PDF", &m.InputCapabilities.PDF, false}, {tr("音频"), &m.InputCapabilities.Audio, true}, {tr("视频"), &m.InputCapabilities.Video, true}} {
									ui.Checkbox(c, option.Value, option.Label).FontSize(p.font(11.48)).Disabled(option.OpenAI && service.APIStyle != "openai")
								}
							})
							muted(c, p, tr("按服务和模型的实际能力勾选；视频需要接口支持视频输入。Anthropic Messages 支持图片与 PDF。")).FontSize(p.font(10.36))
						})
						if button(c, p, tr("移除模型"), "Trash2", false).Clicked() {
							remove = i
						}
					}
				})
			}
			if remove >= 0 {
				service.Models = append(service.Models[:remove], service.Models[remove+1:]...)
				s.OpenModels = map[int]bool{}
			}
		})
	})
}

func (d *desktop) agentSettings(c *ui.Context, p palette, s *settingsState) {
	if s.Agent != nil {
		a := s.Agent
		section(c, p, tr("编辑子助手"), tr("子助手会继承父会话的权限、审批与共享预算。"), func() {
			field(c, p, tr("名称"), &a.Name, false).Disabled(a.Builtin)
			field(c, p, tr("显示名称"), &a.Label, false)
			field(c, p, tr("委派条件"), &a.Description, false)
			ui.Text(c, tr("指令")).FontSize(p.font(11.48)).TextColor(p.Text2)
			ui.TextArea(c, &a.Instructions).Label(tr("子助手指令")).Height(180)
			ui.Checkbox(c, &a.InheritTools, tr("继承父助手全部工具"))
			ui.Row(c).Wrap().Gap(8).Children(func() {
				for _, name := range []string{"read", "ls", "grep", "read_artifact", "bash", "edit", "write"} {
					on := slices.Contains(a.Tools, name)
					if ui.Checkbox(c, &on, name).Disabled(a.InheritTools).Changed() {
						if on {
							a.Tools = append(a.Tools, name)
						} else {
							a.Tools = slices.DeleteFunc(a.Tools, func(t string) bool { return t == name })
						}
					}
				}
			})
			ui.Row(c).Gap(8).Children(func() {
				if button(c, p, tr("保存子助手"), "", true).Clicked() {
					var agents []model.Agent
					d.request(d.current(), "saveAgent", map[string]any{"agent": *a}, &agents, func(err error) {
						if err != nil {
							s.Error = err.Error()
						} else {
							s.Agent = nil
							d.refreshConfiguration()
						}
					})
				}
				if button(c, p, tr("取消"), "", false).Clicked() {
					s.Agent = nil
				}
			})
		})
		return
	}
	section(c, p, tr("子助手"), tr("可复制内置模板或空白创建，编辑委派条件、指令和工具。"), func() {
		if button(c, p, tr("创建子助手"), "Plus", true).Clicked() {
			s.Agent = &model.Agent{Enabled: true, Tools: []string{"read", "ls", "grep"}}
		}
		for _, agent := range d.config.Agents {
			settingsRow(c, p, agent.Label, agent.Description, func() {
				if iconButton(c, p, "Copy", tr("复制 ")+agent.Label).Clicked() {
					copy := agent
					copy.Name = agent.Name + "-custom"
					copy.OriginalName = ""
					copy.Builtin = false
					copy.Tools = slices.Clone(agent.Tools)
					s.Agent = &copy
				}
				if !agent.Builtin && iconButton(c, p, "Pencil", tr("编辑 ")+agent.Label).Clicked() {
					copy := agent
					copy.OriginalName = agent.Name
					copy.Tools = slices.Clone(agent.Tools)
					s.Agent = &copy
				}
				on := agent.Enabled
				if toggle(c, p, &on, tr("启用 ")+agent.Label).Changed() {
					var agents []model.Agent
					d.request(d.current(), "setAgentEnabled", map[string]any{"name": agent.Name, "enabled": on}, &agents, func(err error) {
						if err != nil {
							s.Error = err.Error()
						} else {
							d.refreshConfiguration()
						}
					})
				}
				if !agent.Builtin && iconButton(c, p, "Trash2", tr("删除 ")+agent.Label).Clicked() {
					s.DeleteAgent = agent.Name
				}
			})
			if button(c, p, tr("添加到任务"), "ArrowUpRight", false).Clicked() {
				v := d.current()
				if v.Draft != "" {
					v.Draft += "\n"
				}
				v.Draft += "请使用 " + agent.Name + " 子助手调查并报告证据。"
				d.settings = nil
				d.focusPrompt = true
			}
			if s.DeleteAgent == agent.Name {
				if button(c, p, tr("确认删除子助手"), "Trash2", false).Clicked() {
					var agents []model.Agent
					d.request(d.current(), "deleteAgent", map[string]any{"name": agent.Name}, &agents, func(err error) {
						if err != nil {
							s.Error = err.Error()
						} else {
							s.DeleteAgent = ""
							d.refreshConfiguration()
						}
					})
				}
			}
		}
		for _, name := range d.current().Capabilities.Agents {
			if strings.HasPrefix(name, "plugin:") {
				if button(c, p, tr("添加到任务：")+name, "ArrowUpRight", false).Clicked() {
					d.current().Draft += "请使用 " + name + " 子助手调查并报告证据。"
					d.settings = nil
					d.focusPrompt = true
				}
			}
		}
	})
}
