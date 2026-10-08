package main

import (
	"image/png"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/egoist/mygo/ui"
	"minicode.desktop/internal/bridge"
	"minicode.desktop/internal/model"
)

type testRig struct {
	d     *desktop
	ui    *ui.Tester
	queue chan func()
}

func newTestRig(t *testing.T, page string) *testRig {
	t.Helper()
	r := &testRig{queue: make(chan func(), 1024)}
	r.d = newDesktop(func(fn func()) { r.queue <- fn })
	loadFixture(r.d, page)
	r.d.preferences.Theme = "dark"
	r.ui = ui.NewTester(r.d.view, 1500, 940)
	r.ui.SetPreferences(ui.Preferences{ReduceMotion: true})
	t.Cleanup(func() { r.d.closed.Store(true) })
	return r
}

func (r *testRig) settle() {
	deadline := time.NewTimer(60 * time.Millisecond)
	defer deadline.Stop()
	for {
		select {
		case fn := <-r.queue:
			fn()
			r.ui.Frame()
		case <-deadline.C:
			r.ui.Frame()
			return
		}
	}
}

func click(t *testing.T, tt *ui.Tester, label string) {
	t.Helper()
	if err := tt.Click(label); err != nil {
		t.Fatalf("click %q: %v", label, err)
	}
}

func clickInput(t *testing.T, tt *ui.Tester, label string) {
	t.Helper()
	click(t, tt, label)
	if !tt.Focused(label) {
		rect, ok := tt.Find(label)
		if !ok {
			t.Fatalf("input label %q missing", label)
		}
		tt.ClickAt(rect.X+8, rect.Y+rect.H+18)
	}
	if !tt.Focused(label) {
		t.Fatalf("input %q did not receive focus", label)
	}
}

func TestNativeSnapshots(t *testing.T) {
	for _, page := range []string{"home", "conversation", "approval", "files", "diff", "settings", "settings-model", "settings-subagents", "settings-about"} {
		t.Run(page, func(t *testing.T) {
			r := newTestRig(t, page)
			for _, theme := range []string{"light", "dark"} {
				r.d.preferences.Theme = theme
				r.ui.Frame()
				dir := filepath.Join("..", "..", "output", "native")
				if err := os.MkdirAll(dir, 0755); err != nil {
					t.Fatal(err)
				}
				file, err := os.Create(filepath.Join(dir, page+"-"+theme+"-1x.png"))
				if err != nil {
					t.Fatal(err)
				}
				if err := png.Encode(file, r.ui.Image()); err != nil {
					_ = file.Close()
					t.Fatal(err)
				}
				_ = file.Close()
			}
		})
	}
}

func TestComposerCompletionAndUndo(t *testing.T) {
	r := newTestRig(t, "home")
	click(t, r.ui, "任务输入")
	r.ui.Type("请看 @Side")
	if r.d.completion == nil || len(r.d.completion.Items) == 0 {
		t.Fatal("file completion not shown")
	}
	r.ui.Key(0, ui.KeyTab)
	if got := r.d.current().Draft; got != "请看 @desktop/src/components/Sidebar.tsx " {
		t.Fatalf("completed %q", got)
	}
	r.ui.Command("undo")
	if got := r.d.current().Draft; got != "请看 @Side" {
		t.Fatalf("undo completion: %q", got)
	}
	r.ui.Command("selectAll")
	r.ui.Type("/mo")
	r.ui.Key(0, ui.KeyEnter)
	if got := r.d.current().Draft; got != "/model " {
		t.Fatalf("submenu draft: %q", got)
	}
	if r.d.completion == nil {
		t.Fatal("model submenu missing")
	}
	r.ui.Key(0, ui.KeyEscape)
	if got := r.d.current().Draft; got != "/model " {
		t.Fatalf("Escape erased draft: %q", got)
	}
}

func TestComposerIMEAndSendKeys(t *testing.T) {
	r := newTestRig(t, "home")
	click(t, r.ui, "任务输入")
	r.ui.Compose("zhong", 5)
	r.ui.Key(0, ui.KeyEnter)
	if r.d.current().Busy || len(r.d.current().Items) > 0 {
		t.Fatal("IME Enter submitted a task")
	}
	r.ui.Type("中文任务")
	r.ui.Compose("", 0)
	r.ui.Key(ui.Shift, ui.KeyEnter)
	if got := r.d.current().Draft; got != "中文任务\n" {
		t.Fatalf("Shift Enter: %q", got)
	}
	r.ui.Key(0, ui.KeyEnter)
	if !r.d.current().Busy || r.d.current().Items[0].Text != "中文任务" {
		t.Fatal("Enter did not submit")
	}
	r.settle()
}

func TestQueueRemoveAndCancel(t *testing.T) {
	r := newTestRig(t, "home")
	v := r.d.current()
	v.Busy = true
	v.Queue = []string{"first", "second"}
	r.ui.Frame()
	click(t, r.ui, "取消排队消息 1")
	if len(v.Queue) != 1 || v.Queue[0] != "second" {
		t.Fatalf("queue %v", v.Queue)
	}
	r.d.handleEvent(bridge.Event{Event: "run_done", ClientKey: v.Key, Result: []byte(`{"exit_reason":"cancelled"}`)})
	if v.Busy || len(v.Queue) != 1 {
		t.Fatal("cancel executed a queued task")
	}
	r.settle()
}

func TestSettingsAndModalFocus(t *testing.T) {
	r := newTestRig(t, "home")
	click(t, r.ui, "设置")
	r.settle()
	if r.d.settings == nil {
		t.Fatal("settings not opened")
	}
	click(t, r.ui, "浅色")
	if r.d.preferences.Theme != "light" {
		t.Fatal("theme not changed")
	}
	click(t, r.ui, "技能")
	if !r.ui.HasText("当前工作区没有发现 Skills。") {
		t.Fatal("empty skills state missing")
	}
	click(t, r.ui, "模型与预算")
	click(t, r.ui, "添加服务")
	if r.d.settings.Service == nil {
		t.Fatal("service editor missing")
	}
	clickInput(t, r.ui, "服务名称")
	r.ui.Type("My service")
	clickInput(t, r.ui, "接口地址")
	r.ui.Type("https://example.test/v1")
	clickInput(t, r.ui, "API 密钥")
	r.ui.Type("test-secret")
	if r.ui.HasText("test-secret") {
		t.Fatal("password exposed in text output")
	}
	click(t, r.ui, "自定义模型 ID")
	r.ui.Type("my-model")
	click(t, r.ui, "添加")
	if len(r.d.settings.Service.Models) != 1 {
		t.Fatal("model not added")
	}
	r.ui.Scroll(900, 700, 0, 500)
	click(t, r.ui, "保存服务")
	r.settle()
	if !r.ui.HasText("My service") {
		names := []string{}
		for _, service := range r.d.config.Services {
			names = append(names, service.Name)
		}
		if r.d.settings == nil {
			t.Fatalf("settings closed while editing; names=%v", names)
		}
		t.Fatalf("service was not saved; names=%v editor=%v error=%q", names, r.d.settings.Service != nil, r.d.settings.Error)
	}
	r.ui.Key(0, ui.KeyEscape)
	if r.d.settings != nil {
		t.Fatal("Escape did not close settings")
	}
}

func TestSessionRoutingAndStreaming(t *testing.T) {
	r := newTestRig(t, "home")
	first := r.d.current()
	second := newConversation("other", first.Workspace, initialState())
	second.SessionID = "other-session"
	r.d.views[second.Key] = second
	r.d.handleEvent(bridge.Event{Event: "text_delta", ClientKey: "other", SessionID: "other-session", Text: "Other result"})
	if len(first.Items) != 0 || len(second.Items) != 1 || second.Items[0].Text != "Other result" {
		t.Fatal("stream crossed conversation boundary")
	}
	r.d.handleEvent(bridge.Event{Event: "text_delta", ClientKey: "draft-0", SessionID: "other-session", Text: " more"})
	if len(first.Items) != 0 || second.Items[0].Text != "Other result more" {
		t.Fatal("session fallback routing failed")
	}
	r.d.handleEvent(bridge.Event{Event: "run_done", ClientKey: "other", SessionID: "other-session", Result: []byte(`{"exit_reason":"completed"}`)})
	if !second.Unread {
		t.Fatal("background result unread state missing")
	}
	r.settle()
}

func TestHistoryToolGroupingAndPreview(t *testing.T) {
	r := newTestRig(t, "conversation")
	if !r.ui.HasText("已执行 4 个操作") {
		t.Fatal("tool group missing")
	}
	if !r.ui.HasText(`const [theme, setTheme] = useState("system")`) {
		t.Fatal("edit diff missing")
	}
	click(t, r.ui, "编辑 desktop/src/App.tsx")
	if r.ui.HasText(`const [theme, setTheme] = useState("system")`) {
		t.Fatal("edit details did not collapse")
	}
	click(t, r.ui, "编辑 desktop/src/App.tsx")
	r.d.tab = 3
	r.d.current().Tasks = []model.Task{{ID: "one", Title: "Read changes", Status: "done"}}
	r.ui.Frame()
	if !r.ui.HasText("会话统计") {
		t.Fatal("usage panel missing")
	}
}

func TestMarkdownGFMAndHighlight(t *testing.T) {
	source := "## Heading\n\n**bold** and *italic* and ~~gone~~ and [link](https://example.test)\n\n- [x] done\n- [ ] todo\n\n| A | B |\n| - | - |\n| one | two |\n\n```go\nfunc main() {}\n```\n"
	doc := parseMarkdown(source)
	kinds := map[string]bool{}
	for _, block := range doc.Blocks {
		kinds[block.Kind] = true
	}
	for _, kind := range []string{"heading", "paragraph", "list", "table", "code"} {
		if !kinds[kind] {
			t.Fatalf("%s block absent", kind)
		}
	}
	lines := highlight("const value = 1", "typescript")
	found := false
	for _, line := range lines {
		for _, span := range line {
			if span.Text == "const" {
				found = true
			}
		}
	}
	if !found {
		t.Fatal("code lexer not used")
	}
}

func TestCodeBlockLineLayout(t *testing.T) {
	r := newTestRig(t, "conversation")
	first, ok := r.ui.Find("const active = useRef(true)")
	if !ok {
		t.Fatal("first code line missing")
	}
	second, ok := r.ui.Find("if (active.current) setContent(result)")
	if !ok || second.Y <= first.Y {
		t.Fatal("code lines must be vertically stacked")
	}
}

func TestNativeKeyboardAndContextMenus(t *testing.T) {
	r := newTestRig(t, "home")
	r.ui.Key(ui.Cmd, ui.KeyB)
	if !r.d.preferences.LeftCollapsed {
		t.Fatal("sidebar shortcut failed")
	}
	r.ui.Key(ui.Cmd|ui.Shift, ui.KeyB)
	if !r.d.preferences.RightCollapsed {
		t.Fatal("work panel shortcut failed")
	}
	r.ui.Key(ui.Cmd, ui.Key2)
	if r.d.tab != 1 || r.d.preferences.RightCollapsed {
		t.Fatal("work tab shortcut failed")
	}
	r.ui.Key(ui.Cmd, ui.KeyB)
	if err := r.ui.RightClick("审查工作区的未提交改动"); err != nil {
		t.Fatal(err)
	}
	if err := r.ui.ChooseMenuItem("重命名"); err != nil {
		t.Fatal(err)
	}
	if r.d.renameID != "audit" {
		t.Fatal("rename context menu failed")
	}
	r.d.renameID = ""
	r.ui.Key(ui.Cmd, ui.KeyK)
	if r.d.modal != "palette" {
		t.Fatal("palette shortcut failed")
	}
	r.ui.Key(0, ui.KeyEscape)
	if r.d.modal != "" {
		t.Fatal("palette escape failed")
	}
	r.ui.SetSize(1030, 680)
	r.ui.SetScale(1.5)
	if !r.ui.HasText("MiniCode") {
		t.Fatal("minimum-size high-DPI layout missing")
	}
}

func TestNativeUpdateControls(t *testing.T) {
	r := newTestRig(t, "home")
	u := r.d.updates.(*fixtureUpdates)
	click(t, r.ui, "设置")
	r.settle()
	r.ui.Scroll(900, 650, 0, 600)
	click(t, r.ui, "启用自动检查更新")
	if u.checks {
		t.Fatal("automatic checks preference not changed")
	}
	click(t, r.ui, "启用自动下载并安装")
	if !u.downloads {
		t.Fatal("automatic downloads preference not changed")
	}
	click(t, r.ui, "关于")
	click(t, r.ui, "检查更新")
	if u.count != 1 {
		t.Fatal("manual update check not dispatched")
	}
}
