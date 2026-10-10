package main

import (
	"context"
	"encoding/json"
	"strings"
	"sync"
	"time"

	"minicode.desktop/internal/model"
)

type fixtureClient struct {
	mu       sync.Mutex
	State    model.State
	Sessions []model.Session
	Models   []model.Model
	Config   model.Configuration
	Detail   model.Detail
}

func (f *fixtureClient) Request(_ context.Context, method string, params any, target any) error {
	f.mu.Lock()
	defer f.mu.Unlock()
	raw, _ := json.Marshal(params)
	var p map[string]any
	_ = json.Unmarshal(raw, &p)
	var result any = f.State
	switch method {
	case "initialize":
		result = map[string]any{"sessions": f.Sessions, "models": f.Models, "state": f.State, "commands": fixtureCommands()}
	case "listSessions":
		result = f.Sessions
	case "listModels":
		result = f.Models
	case "getConfiguration":
		result = f.Config
	case "getSession":
		detail := f.Detail
		id := model.String(p["sessionId"])
		for _, s := range f.Sessions {
			if s.ID == id {
				detail.Summary = s
				break
			}
		}
		result = detail
	case "selectSession":
		f.State.SessionID = model.String(p["sessionId"])
		result = f.State
	case "resetSession":
		f.State.SessionID = ""
		result = f.State
	case "setModel":
		f.State.Model = model.String(p["model"])
		result = f.State
	case "setEffort":
		f.State.Effort = model.String(p["effort"])
		result = f.State
	case "setPermissionMode":
		f.State.PermissionMode = model.String(p["mode"])
		result = f.State
	case "listTasks":
		result = []model.Task{{ID: "t1", Title: "读取改动文件", Status: "done"}, {ID: "t2", Title: "运行构建", Status: "done"}, {ID: "t3", Title: "整理结论", Status: "running"}}
	case "getCapabilities":
		result = model.Capabilities{Agents: []string{"explore", "review"}}
	case "sendPrompt":
		result = map[string]string{"sessionId": "test-run"}
	case "cancelTurn", "resolveApproval", "clearAlwaysAllow":
		result = true
	case "renameSession", "pinSession", "deleteSession":
		id := model.String(p["sessionId"])
		for i := range f.Sessions {
			if f.Sessions[i].ID == id {
				switch method {
				case "renameSession":
					f.Sessions[i].Title = model.String(p["title"])
				case "pinSession":
					f.Sessions[i].Pinned = model.Bool(p["pinned"])
				case "deleteSession":
					f.Sessions = append(f.Sessions[:i], f.Sessions[i+1:]...)
				}
				break
			}
		}
		result = f.Sessions
	case "setDefaultModel":
		f.Config.DefaultModel = model.String(p["model"])
		result = f.Config
	case "saveService":
		raw, _ := json.Marshal(p["service"])
		var service model.Service
		_ = json.Unmarshal(raw, &service)
		if service.ID == "" {
			service.ID = "fixture-service"
		}
		found := false
		for i := range f.Config.Services {
			if f.Config.Services[i].ID == service.ID {
				f.Config.Services[i] = service
				found = true
			}
		}
		if !found {
			f.Config.Services = append(f.Config.Services, service)
		}
		service.APIKey = ""
		result = f.Config
	case "deleteService":
		id := model.String(p["id"])
		for i := range f.Config.Services {
			if f.Config.Services[i].ID == id {
				f.Config.Services = append(f.Config.Services[:i], f.Config.Services[i+1:]...)
				break
			}
		}
		result = f.Config
	case "fetchServiceModels", "testServiceConnection":
		result = map[string]any{"models": []model.ServiceModel{{ModelID: "test-model", Name: "Test model", ContextWindow: 200000, MaxOutput: 8192}}, "message": "已连接，发现 1 个模型"}
	case "refreshMcp":
		result = model.Discovery{}
	case "saveAgent", "deleteAgent", "setAgentEnabled":
		result = f.Config.Agents
	case "readArtifact":
		result = map[string]any{"text": "完整归档输出\n", "total": 7, "hasMore": false}
	case "runSlash":
		result = map[string]any{"message": "命令已执行", "state": f.State}
	}
	if target == nil {
		return nil
	}
	data, _ := json.Marshal(result)
	return json.Unmarshal(data, target)
}

type fixtureFiles struct{}

func (fixtureFiles) Files(_ context.Context, _ string, _ string) ([]string, error) {
	return fixturePaths(), nil
}
func (fixtureFiles) Changes(_ context.Context, _ string) ([]model.Change, error) {
	return fixtureChanges(), nil
}
func (fixtureFiles) Diff(_ context.Context, _ string, _ string) (string, error) {
	return fixtureDiff(), nil
}
func (fixtureFiles) Stage(context.Context, string, string) error   { return nil }
func (fixtureFiles) StageAll(context.Context, string) error        { return nil }
func (fixtureFiles) Unstage(context.Context, string, string) error { return nil }

func fixturePaths() []string {
	return []string{"desktop/src/App.tsx", "desktop/src/styles/index.css", "desktop/src/components/Sidebar.tsx", "desktop/src/components/feed/SessionFeed.tsx", "desktop/src/components/work/WorkPanel.tsx", "desktop/bridge.py", "README.md"}
}
func fixtureChanges() []model.Change {
	return []model.Change{{Path: "desktop/src/App.tsx", Status: " M", Additions: 12, Deletions: 3}, {Path: "desktop/src/styles/index.css", Status: "??", Additions: 40}, {Path: "desktop/src/components/Sidebar.tsx", Status: "M ", Additions: 5, Deletions: 5}}
}
func fixtureCommands() []model.Command {
	return []model.Command{{Name: "/help", Usage: "/help", Summary: "查看命令"}, {Name: "/model", Usage: "/model <模型>", Summary: "切换模型"}, {Name: "/effort", Usage: "/effort <强度>", Summary: "思考强度"}, {Name: "/permissions", Usage: "/permissions <模式>", Summary: "权限模式"}, {Name: "/new", Summary: "新建任务"}, {Name: "/clear", Summary: "清空视图"}, {Name: "/compact", Summary: "压缩上下文"}, {Name: "/resume", Summary: "恢复会话"}, {Name: "/continue", Summary: "继续任务"}, {Name: "/skill", Summary: "加载技能"}}
}
func fixtureDiff() string {
	return "diff --git a/desktop/src/App.tsx b/desktop/src/App.tsx\n--- a/desktop/src/App.tsx\n+++ b/desktop/src/App.tsx\n@@ -12,3 +12,4 @@\n export default function App() {\n-  const [theme] = useState(\"dark\")\n+  const [theme, setTheme] = useState(\"system\")\n+  // Follow the system appearance\n }"
}

func loadFixture(d *desktop, page string) {
	d.updates = &fixtureUpdates{checks: true}
	now := time.Now()
	root := "D:\\miniclaudecode"
	sessions := []model.Session{
		{ID: "audit", Title: "审查工作区的未提交改动", Workspace: root, Status: "completed", CreatedAt: now.Add(-time.Hour).Format(time.RFC3339), UpdatedAt: now.Add(-10 * time.Minute).Format(time.RFC3339)},
		{ID: "bridge", Title: "完善 Agent 事件流与任务进度", Workspace: root, Status: "completed", CreatedAt: now.Add(-24 * time.Hour).Format(time.RFC3339), UpdatedAt: now.Add(-24 * time.Hour).Format(time.RFC3339), Pinned: true},
		{ID: "terminal", Title: "排查终端输出的刷新问题", Workspace: root, Status: "paused", CreatedAt: now.Add(-48 * time.Hour).Format(time.RFC3339), UpdatedAt: now.Add(-48 * time.Hour).Format(time.RFC3339)},
	}
	models := []model.Model{{ID: "fake", Provider: "offline", Available: true}, {ID: "deepseek/deepseek-v4.1-flash", Provider: "commandcode", Available: true, SupportsEffort: true}, {ID: "claude-sonnet-4-6", Provider: "anthropic"}}
	state := initialState()
	state.Model = "fake"
	state.ContextTokens = 18400
	state.Breakdown = map[string]int{"system": 6200, "tools": 9800, "messages": 2400}
	state.Usage = &model.Usage{Input: 42000, Output: 3100, Available: true}
	assistant := "### 审查结果\n\n已检查事件流与界面更新，主要涉及以下文件：\n\n- `App.tsx`：流式消息按批更新，降低渲染频率。\n- `WorkPanel.tsx`：切换文件时丢弃过期的读取结果。\n\n```tsx\nconst active = useRef(true)\nif (active.current) setContent(result)\n```\n\n| 文件 | 结论 |\n| --- | --- |\n| App.tsx | 通过 |\n\n建议继续检查**连续切换工作区**时的任务状态。"
	items := []*model.FeedItem{
		{ID: "user-1", Kind: "user", Text: "帮我审查当前工作区的改动，重点关注事件流与界面交互。"},
		{ID: "c1", Kind: "tool", Name: "read", Args: map[string]any{"path": "desktop/src/App.tsx"}, Output: "1\texport default function App() {", Success: true},
		{ID: "c2", Kind: "tool", Name: "grep", Args: map[string]any{"pattern": "useState", "path": "desktop/src"}, Output: "desktop/src/App.tsx:12: useState", Success: true},
		{ID: "c3", Kind: "tool", Name: "edit", Args: map[string]any{"path": "desktop/src/App.tsx", "old_text": `const [theme] = useState("dark")`, "new_text": "const [theme, setTheme] = useState(\"system\")\n// Follow the system appearance"}, Output: "edited desktop/src/App.tsx", Success: true},
		{ID: "c4", Kind: "tool", Name: "bash", Args: map[string]any{"command": "npm run build"}, Output: "> tsc --noEmit && vite build\n✓ built in 3.2s", Success: true},
		{ID: "assistant-1", Kind: "assistant", Text: assistant},
	}
	config := model.Configuration{Services: []model.Service{{ID: "commandcode", Name: "CommandCode", BaseURL: "https://api.commandcode.ai/provider/v1", APIStyle: "openai", Enabled: true, HasAPIKey: true, Models: []model.ServiceModel{{ModelID: "deepseek/deepseek-v4.1-flash", Name: "DeepSeek", ContextWindow: 200000, MaxOutput: 8192, SupportsEffort: true}}}}, Agents: []model.Agent{{Name: "explore", Label: "探索者", Description: "调查项目结构、定位文件与实现。", Instructions: "检查项目并报告证据。", Builtin: true, Enabled: true, Tools: []string{"read", "ls", "grep"}}, {Name: "review", Label: "代码审查员", Description: "检查改动中的问题与回归。", Builtin: true, Enabled: true}}}
	d.client = &fixtureClient{State: state, Sessions: sessions, Models: models, Config: config,
		Detail: model.Detail{Summary: sessions[0], Messages: []model.Message{{Role: "user", Content: []map[string]any{{"type": "text", "text": items[0].Text}}}, {Role: "assistant", Content: []map[string]any{{"type": "text", "text": assistant}}}}}}
	d.files = fixtureFiles{}
	d.root = root
	d.recent = []string{root}
	d.sessions = sessions
	d.models = models
	d.commands = fixtureCommands()
	d.config = config
	v := d.current()
	v.Workspace = root
	v.State = state
	v.Loaded = true
	v.Files = fixturePaths()
	v.Changes = fixtureChanges()
	switch {
	case page == "model-menu":
		v.State.Model = models[1].ID
		v.State.Effort = "high"
		d.modelOpen = true
	case page == "permission-menu":
		d.permissionOpen = true
	case page == "collapsed":
		d.preferences.LeftCollapsed = true
		d.preferences.RightCollapsed = true
	case page == "settings-empty":
		d.models = nil
		d.config.Services = nil
		v.State.Model = ""
		d.settings = &settingsState{Tab: "model", Budget: [3]string{"0", "0", "0"}, OpenModels: map[int]bool{}}
	case page == "conversation" || page == "approval":
		v.SessionID = "audit"
		v.State.SessionID = "audit"
		v.Items = items
		v.rebuildRows()
		if page == "conversation" {
			d.tab = 3
			v.Tasks = []model.Task{{ID: "t1", Title: "读取改动文件", Status: "done"}, {ID: "t2", Title: "运行构建", Status: "done"}, {ID: "t3", Title: "整理结论", Status: "running"}}
			v.Open["group-"+v.Rows[1].ID] = true
			v.Open["tool-c3"] = true
			v.State.Model = "deepseek/deepseek-v4.1-flash"
			v.State.Effort = "high"
			v.State.PermissionMode = "accept_edits"
		}
		if page == "approval" {
			v.Busy = true
			v.StartedAt = now.Add(-5 * time.Second)
			v.append(&model.FeedItem{ID: "approval-1", Kind: "approval", Name: "bash", Args: map[string]any{"command": "python -m pytest tests -q"}, ApprovalID: "approval-1"})
		}
	case page == "files":
		d.tab = 1
		d.folders["desktop"] = true
		d.folders["desktop/src"] = true
		d.folders["desktop/src/components"] = true
	case page == "diff":
		d.preview = &previewState{Kind: "diff", Path: "desktop/src/App.tsx", Status: " M", Text: fixtureDiff(), Diff: convertDiff(fixtureDiff()), Expanded: map[int]bool{}}
	case strings.HasPrefix(page, "settings"):
		tab := "general"
		if suffix := strings.TrimPrefix(page, "settings-"); suffix != page {
			tab = suffix
		}
		d.settings = &settingsState{Tab: tab, Budget: [3]string{"0", "0", "0"}, OpenModels: map[int]bool{}}
	}
}

type fixtureUpdates struct {
	checks, downloads bool
	count             int
}

func (*fixtureUpdates) Enabled() bool                   { return true }
func (u *fixtureUpdates) AutomaticChecks() bool         { return u.checks }
func (u *fixtureUpdates) AutomaticDownloads() bool      { return u.downloads }
func (u *fixtureUpdates) SetAutomaticChecks(on bool)    { u.checks = on }
func (u *fixtureUpdates) SetAutomaticDownloads(on bool) { u.downloads = on }
func (*fixtureUpdates) LastCheck() time.Time            { return time.Time{} }
func (u *fixtureUpdates) Check()                        { u.count++ }
