package main

import (
	"fmt"
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
	for _, page := range []string{"home", "conversation", "approval", "files", "diff", "settings", "settings-model", "settings-subagents", "settings-about", "settings-permissions", "settings-empty", "model-menu", "permission-menu", "collapsed"} {
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

func TestPanelControlsAndCompactRail(t *testing.T) {
	r := newTestRig(t, "home")
	if _, ok := r.ui.Find("搜索"); ok {
		t.Fatal("removed header controls still visible")
	}
	rect, ok := r.ui.Find("收拢左侧面板")
	if !ok || rect.Y < 40 || rect.X < r.d.preferences.SidebarWidth-48 {
		t.Fatalf("collapse control outside sidebar top right: %+v", rect)
	}
	click(t, r.ui, "收拢左侧面板")
	rect, ok = r.ui.Find("展开左侧面板")
	if !ok || rect.X > 44 || rect.Y > 48 {
		t.Fatalf("collapsed control not at rail top: %+v", rect)
	}
	click(t, r.ui, "展开左侧面板")
	if r.d.preferences.LeftCollapsed {
		t.Fatal("sidebar did not reopen")
	}
	r.d.preferences.RightCollapsed = true
	r.ui.Frame()
	rect, ok = r.ui.Find("展开工作区")
	if !ok || rect.X < 1460 {
		t.Fatalf("work rail reserves excess width: %+v", rect)
	}
	click(t, r.ui, "展开工作区")
	if r.d.preferences.RightCollapsed {
		t.Fatal("work rail did not reopen")
	}
}

func TestPermissionChoicesAlignAndApply(t *testing.T) {
	for _, page := range []string{"settings-permissions", "permission-menu"} {
		t.Run(page, func(t *testing.T) {
			r := newTestRig(t, page)
			var labelX float32
			for i, option := range permissionOptions {
				rect, ok := r.ui.Find(option.Detail)
				if !ok || (i > 0 && rect.X != labelX) {
					t.Fatalf("permission descriptions do not align: %+v", rect)
				}
				labelX = rect.X
			}
			click(t, r.ui, "自动编辑")
			r.settle()
			if r.d.current().State.PermissionMode != "accept_edits" {
				t.Fatal("permission did not reach bridge")
			}
		})
	}
}

func TestEffortSliderPointerKeyboardAndDisabled(t *testing.T) {
	value, supported, commits := "off", true, 0
	tt := ui.NewTester(func(c *ui.Context) {
		ui.Column(c).Width(320).Padding(8).Children(func() {
			effortSlider(c, colors(false), value, supported, func(next string) { value = next; commits++ })
		})
	}, 360, 150)
	tt.SetPreferences(ui.Preferences{ReduceMotion: true})
	rect, ok := tt.Find("思考强度滑条")
	if !ok {
		t.Fatal("slider missing")
	}
	y := rect.Y + rect.H/2
	tt.Press(rect.X+12, y)
	tt.Move(rect.X+rect.W-12, y)
	if commits != 0 {
		t.Fatal("drag sent intermediate bridge updates")
	}
	tt.Release(rect.X+rect.W-12, y)
	if value != "max" || commits != 1 {
		t.Fatalf("drag result %s, commits %d", value, commits)
	}
	tt.Key(0, ui.KeyLeft)
	if value != "xhigh" {
		t.Fatal("arrow key did not step effort")
	}
	click(t, tt, "思考强度：中")
	if value != "medium" {
		t.Fatal("tick alternative did not apply")
	}
	supported = false
	tt.Frame()
	click(t, tt, "思考强度：最大")
	if value != "medium" {
		t.Fatal("disabled slider changed effort")
	}
}

func TestModelMenuKeepsEffortAvailableAfterSwitch(t *testing.T) {
	r := newTestRig(t, "model-menu")
	click(t, r.ui, "切换至 fake")
	r.settle()
	if r.d.current().State.Model != "fake" || !r.d.modelOpen {
		t.Fatal("model switch failed or closed controls")
	}
	if !r.ui.HasText("不可用") {
		t.Fatal("unsupported model effort not disabled")
	}
	click(t, r.ui, "切换至 deepseek/deepseek-v4.1-flash")
	r.settle()
	click(t, r.ui, "思考强度：中")
	r.settle()
	if r.d.current().State.Effort != "medium" {
		t.Fatal("menu effort did not reach bridge")
	}
}

func TestChoiceSurfacesAtMinimumWindowAndLargeText(t *testing.T) {
	for _, page := range []string{"model-menu", "permission-menu", "settings-permissions", "settings-empty"} {
		t.Run(page, func(t *testing.T) {
			r := newTestRig(t, page)
			r.ui.SetSize(1030, 680)
			r.ui.SetScale(1.5)
			r.ui.SetPreferences(ui.Preferences{ReduceMotion: true, TextScale: 1.2})
			labels := []string{"自动编辑", "全部允许"}
			if page == "model-menu" {
				labels = []string{"思考强度滑条", "管理模型与服务"}
			} else if page == "settings-empty" {
				labels = []string{"添加服务"}
			}
			for _, label := range labels {
				rect, ok := r.ui.Find(label)
				if !ok || rect.X < 0 || rect.Y < 0 || rect.X+rect.W > 1030 || rect.Y+rect.H > 680 {
					t.Fatalf("%s is clipped at minimum size: %+v", label, rect)
				}
			}
		})
	}
}

func TestModelMenuManyModelsFitsWindow(t *testing.T) {
	for _, size := range [][2]float32{{1030, 680}, {1500, 940}} {
		r := newTestRig(t, "home")
		r.ui.SetSize(int(size[0]), int(size[1]))
		r.ui.SetPreferences(ui.Preferences{ReduceMotion: true, TextScale: 1.2})
		for i := 0; i < 20; i++ {
			r.d.models = append(r.d.models, model.Model{ID: fmt.Sprintf("model-%d", i), Name: fmt.Sprintf("Model %d", i), Available: true})
		}
		click(t, r.ui, "切换模型")
		for _, label := range []string{"思考强度滑条", "管理模型与服务"} {
			rect, ok := r.ui.Find(label)
			if !ok || rect.W <= 0 || rect.H <= 0 || rect.Y < 0 || rect.Y+rect.H > size[1] {
				t.Fatalf("%s outside window %+v at %v", label, rect, size)
			}
		}
		list, _ := r.ui.Find("切换至 fake")
		r.ui.Scroll(list.X+20, list.Y+10, 0, 1500)
		click(t, r.ui, "切换至 model-19")
		r.settle()
		if r.d.current().State.Model != "model-19" {
			t.Fatal("last model cannot be selected")
		}
	}
}

func TestContextShowsBothSpeedMeasurements(t *testing.T) {
	r := newTestRig(t, "home")
	r.ui.SetSize(1030, 680)
	generation, request, wait := 300.0, 30.0, 5.0
	stats := &model.Statistics{TPS: &generation, LastTPS: &generation, RequestTPS: &request, Samples: []model.UsageSample{{FirstTokenSeconds: &wait}}}
	r.d.current().State.Statistics = stats
	r.d.client.(*fixtureClient).State.Statistics = stats
	click(t, r.ui, "上下文窗口详情")
	r.settle()
	for _, label := range []string{"请求整体速度", "最近首字等待", "300.0 tok/s", "30.0 tok/s", "5.00 秒"} {
		rect, ok := r.ui.Find(label)
		if !ok || rect.W <= 0 || rect.H <= 0 || rect.Y < 0 || rect.Y+rect.H > 680 {
			t.Fatalf("speed metric hidden: %s %+v", label, rect)
		}
	}
}

func TestBusyStatusDoesNotWrapAndServiceEditorStaysUsable(t *testing.T) {
	r := newTestRig(t, "conversation")
	r.ui.SetSize(1030, 680)
	r.ui.SetPreferences(ui.Preferences{ReduceMotion: true, TextScale: 1.2})
	v := r.d.current()
	v.Busy = true
	v.StartedAt = time.Now().Add(-65 * time.Second)
	r.ui.Frame()
	for _, label := range []string{"任务处理中", "1 分 5 秒"} {
		rect, ok := r.ui.Find(label)
		if !ok || rect.H > 24 || rect.W <= 0 {
			t.Fatalf("busy status wraps or vanishes: %s %+v", label, rect)
		}
	}
	r.d.openSettings("model")
	r.settle()
	click(t, r.ui, "编辑 CommandCode")
	s := r.d.settings
	s.OpenModels[0] = true
	s.Service.Models[0].ContextWindow = 1048576
	s.Service.Models[0].MaxOutput = 524288
	r.ui.Frame()
	for _, label := range []string{"服务名称", "保存服务", "取消"} {
		rect, ok := r.ui.Find(label)
		if !ok || rect.W <= 0 || rect.H <= 0 || rect.Y+rect.H > 680 {
			t.Fatalf("service action missing: %s %+v", label, rect)
		}
	}
	r.ui.Scroll(800, 400, 0, 1000)
	for _, label := range []string{"上下文窗口", "输出上限"} {
		rect, ok := r.ui.Find(label)
		if !ok || rect.W < 180 || rect.Y+rect.H > 610 {
			t.Fatalf("number field too narrow or clipped: %s %+v", label, rect)
		}
	}
	clickInput(t, r.ui, "上下文窗口")
	r.ui.Key(ui.Cmd, ui.KeyA)
	r.ui.Type("2097152")
	if s.Service.Models[0].ContextWindow != 2097152 {
		t.Fatal("context number edit did not apply")
	}
	file, err := os.Create(filepath.Join("..", "..", "output", "native", "settings-service-large-text.png"))
	if err != nil {
		t.Fatal(err)
	}
	defer file.Close()
	if err := png.Encode(file, r.ui.Image()); err != nil {
		t.Fatal(err)
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
