package main

import (
	"testing"

	"github.com/egoist/mygo/ui"
)

func TestThemePresetsResolutionAndCycle(t *testing.T) {
	if proseFont("") != sansFont || proseFont("sans") != sansFont {
		t.Fatal("default prose font must be sansFont")
	}
	if proseFont("serif") != serifFont {
		t.Fatal("serif option must still resolve to serifFont")
	}
	if proseFont("system") != systemFont {
		t.Fatal("system option must resolve to systemFont")
	}

	if got := resolveThemePreset("system", "", true); got != "mono-carbon" {
		t.Fatalf("system dark resolved to %q", got)
	}
	if got := resolveThemePreset("system", "t3-noir", true); got != "t3-noir" {
		t.Fatalf("remembered dark preset ignored: %q", got)
	}
	if got := resolveThemePreset("system", "t3-noir", false); got != "daylight-paper" {
		t.Fatalf("system light resolved to %q", got)
	}
	if got := resolveThemePreset("light", "", true); got != "daylight-paper" {
		t.Fatalf("light resolved to %q", got)
	}

	seenApp := map[ui.Color]string{}
	for _, preset := range themePresets {
		p := paletteFor(preset.ID)
		if p.Preset != preset.ID {
			t.Fatalf("palette preset mismatch: %q != %q", p.Preset, preset.ID)
		}
		if p.Chrome == (ui.Color{}) || p.SecondaryAccent == (ui.Color{}) || p.BorderSpecular == (ui.Color{}) {
			t.Fatalf("preset %q missing semantic colors", preset.ID)
		}
		if prev, dup := seenApp[p.App]; dup {
			t.Fatalf("preset %q shares App color with %q", preset.ID, prev)
		}
		seenApp[p.App] = preset.ID
		if p.Dark != (preset.ID != "daylight-paper") {
			t.Fatalf("preset %q dark flag mismatch: %v", preset.ID, p.Dark)
		}
	}

	r := newTestRig(t, "home")
	r.d.preferences.Theme = "mono-carbon"
	r.ui.Frame()
	wantOrder := []string{"t3-noir", "warm-ink", "daylight-paper", "mono-carbon"}
	for _, want := range wantOrder {
		r.d.toggleTheme()
		if r.d.preferences.Theme != want {
			t.Fatalf("toggleTheme gave %q, want %q", r.d.preferences.Theme, want)
		}
	}
}

func TestTitlebarTabsTelemetryAndShortcuts(t *testing.T) {
	r := newTestRig(t, "conversation")
	r.ui.Frame()
	for _, text := range []string{"D3D11 · v3.0", "TPS", "Cache", "84.2", "92%", "⌘1", "⌘2", "⌘3"} {
		if !r.ui.HasText(text) {
			t.Fatalf("titlebar missing %q", text)
		}
	}
	r.ui.Key(ui.Alt, ui.Key2)
	if r.d.current().SessionID != "bridge" {
		t.Fatalf("Alt+2 switched to %q, want bridge", r.d.current().SessionID)
	}
	r.ui.Key(ui.Cmd, ui.KeyT)
	if r.d.current().SessionID != "" {
		t.Fatalf("Cmd+T did not open a new draft tab")
	}
}

func TestSidebarV2InboxAndTreeModes(t *testing.T) {
	r := newTestRig(t, "conversation")
	r.ui.Frame()
	for _, text := range []string{"需要处理 · ACTIVE", "已归档就绪 · SETTLED", "wt/ui-v3", "wt/bridge-bench", "+64", "−11"} {
		if !r.ui.HasText(text) {
			t.Fatalf("Inbox sidebar missing %q", text)
		}
	}
	click(t, r.ui, "工作区目录树")
	if r.d.preferences.SidebarMode != "tree" || !r.ui.HasText("工作空间") {
		t.Fatal("failed to switch sidebar to workspace tree mode")
	}
	click(t, r.ui, "Inbox 线程流")
	if r.d.preferences.SidebarMode != "inbox" || !r.ui.HasText("需要处理 · ACTIVE") {
		t.Fatal("failed to switch sidebar back to Inbox mode")
	}
}

func TestComposerOperatorChipAndPinnedContextBar(t *testing.T) {
	r := newTestRig(t, "conversation")
	r.ui.Frame()
	if !r.ui.HasText("/operator") || !r.ui.HasText("wt/ui-v3") || !r.ui.HasText("· 3 处改动") {
		t.Fatal("composer toolbar or pinned context bar missing expected elements")
	}
	click(t, r.ui, "子代理编排")
	if r.d.current().Draft != "/operator " {
		t.Fatalf("operator chip set draft to %q", r.d.current().Draft)
	}
	r.d.current().Draft = ""
	r.d.tab = 2
	r.d.preferences.RightCollapsed = true
	r.ui.Frame()
	click(t, r.ui, "查看改动")
	if r.d.tab != 0 || r.d.preferences.RightCollapsed {
		t.Fatalf("diff stats button did not open changes tab: tab=%d collapsed=%v", r.d.tab, r.d.preferences.RightCollapsed)
	}

	v := r.d.current()
	before := len(v.Items)
	r.d.slash("/operator", v)
	if len(v.Items) != before+1 || v.Busy {
		t.Fatal("empty /operator should post usage notice without starting a run")
	}
	r.d.slash("/operator 检查渲染性能", v)
	if !v.Busy {
		t.Fatal("/operator with task should dispatch prompt")
	}
	r.settle()
}

func TestDualColumnDiffHandoffAndApprovalKeycaps(t *testing.T) {
	r := newTestRig(t, "approval")
	r.ui.Frame()
	for _, text := range []string{"PERMISSION HARNESS", "desktop/native/theme.go", "按键盘快捷键可直接响应审批", "N", "A", "P", "Y"} {
		if !r.ui.HasText(text) {
			t.Fatalf("approval card missing %q", text)
		}
	}
	// Context line 1 in Myers diff has both old line 1 and new line 1? Here line 1 is removed ("t.Radius = 10") and line 1/2 are added.
	if !r.ui.HasText("1") || !r.ui.HasText("2") {
		t.Fatal("dual-column line numbers missing in approval diff")
	}

	h := newTestRig(t, "handoff")
	h.ui.Frame()
	for _, text := range []string{"MonoCode Handoff", "SPLIT READY", "查看子会话"} {
		if !h.ui.HasText(text) {
			t.Fatalf("handoff card missing %q", text)
		}
	}
	click(t, h.ui, "查看子会话")
	if h.d.current().SessionID != "bridge" {
		t.Fatalf("handoff jump went to %q, want bridge", h.d.current().SessionID)
	}
}

func TestRightWorkbenchGitCardAndTelemetryBento(t *testing.T) {
	r := newTestRig(t, "conversation")
	r.ui.Frame()
	for _, text := range []string{"wt/ui-v3", "1 个已暂存", "3 个变更文件", "刷新改动", "全部暂存"} {
		if !r.ui.HasText(text) {
			t.Fatalf("Tab 0 Git card missing %q", text)
		}
	}
	for tab := range workTabs {
		r.d.tab = tab
		r.ui.Frame()
		for _, metric := range []string{"输入 / 缓存命中", "输出吞吐 TPS", "最近回合耗时", "工作区改动"} {
			if !r.ui.HasText(metric) {
				t.Fatalf("telemetry bento metric %q missing on tab %d", metric, tab)
			}
		}
	}
}
