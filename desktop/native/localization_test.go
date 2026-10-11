package main

import (
	"encoding/json"
	"go/ast"
	"go/parser"
	"go/token"
	"image/png"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"testing"
)

func TestNativeLanguagePreferenceMigrationAndPersistence(t *testing.T) {
	t.Cleanup(func() { setUILanguage("zh") })
	if defaultPreferences().Language != "zh" {
		t.Fatal("existing installations must default to Chinese")
	}
	for _, saved := range []string{`{"preferences":{"theme":"dark"}}`, `{"preferences":{"language":"invalid","theme":"dark"}}`, `{"preferences":{"language":"en","theme":"dark"}}`} {
		dir := t.TempDir()
		path := filepath.Join(dir, "native-preferences.json")
		if err := os.WriteFile(path, []byte(saved), 0600); err != nil {
			t.Fatal(err)
		}
		d := newDesktop(func(fn func()) { fn() })
		d.loadPreferences(dir)
		want := "zh"
		if strings.Contains(saved, `"en"`) {
			want = "en"
		}
		if d.preferences.Language != want || d.preferences.Theme != "dark" {
			t.Fatalf("migration changed preferences: %+v", d.preferences)
		}
		if storedUILanguage(dir) != want {
			t.Fatal("updater and desktop language preferences differ")
		}
		d.preferences.Language = "en"
		d.persist()
		d.store.timer.Stop()
		d.store.flush()
		var stored savedPreferences
		data, _ := os.ReadFile(path)
		if err := json.Unmarshal(data, &stored); err != nil || stored.Preferences.Language != "en" {
			t.Fatalf("English preference not persisted: %s", data)
		}
		restored := newDesktop(func(fn func()) { fn() })
		restored.loadPreferences(dir)
		if restored.preferences.Language != "en" || tr("设置") != "Settings" {
			t.Fatal("stored language not restored")
		}
	}
}

func TestNativeLanguageSwitchKeepsDraftAndAppliesImmediately(t *testing.T) {
	t.Cleanup(func() { setUILanguage("zh") })
	r := newTestRig(t, "settings")
	r.d.current().Draft = "保留自定义指令 /keep and @AGENTS.md"
	r.ui.Frame()
	click(t, r.ui, "English")
	if r.d.preferences.Language != "en" {
		t.Fatal("language switch did not update preference")
	}
	r.ui.Frame()
	for _, label := range []string{"Settings", "Close settings", "Interface language", "Theme", "New conversation", "Search conversations"} {
		if _, ok := r.ui.Find(label); !ok {
			t.Errorf("English interface control %q missing", label)
		}
	}
	if r.d.current().Draft != "保留自定义指令 /keep and @AGENTS.md" {
		t.Fatal("changing UI language modified the prompt")
	}
	click(t, r.ui, "中文")
	r.ui.Frame()
	if r.d.preferences.Language != "zh" {
		t.Fatal("could not switch back to Chinese")
	}
	if _, ok := r.ui.Find("关闭设置"); !ok {
		t.Fatal("Chinese labels were not restored")
	}
}

func TestNativeLanguageEnglishSnapshotsAndControlCoverage(t *testing.T) {
	t.Cleanup(func() { setUILanguage("zh") })
	pages := []struct {
		page  string
		label string
	}{
		{"home", "New conversation"}, {"collapsed", "Expand sidebar"},
		{"settings", "Interface language"}, {"settings-model", "Current conversation model"},
		{"settings-permissions", "Tool permissions"}, {"permission-menu", "Permission mode"},
		{"model-menu", "Switch model"}, {"reasoning", "Reasoning"},
		{"settings-instructions", "Global instructions"}, {"settings-skills", "Search local resources"},
		{"settings-mcp", "Search local resources"}, {"settings-plugins", "Search local resources"},
	}
	for _, page := range pages {
		t.Run(page.page, func(t *testing.T) {
			r := newTestRig(t, page.page)
			r.d.preferences.Language = "en"
			for _, theme := range []string{"light", "dark"} {
				r.d.preferences.Theme = theme
				r.ui.Frame()
				r.settle()
				if _, ok := r.ui.Find(page.label); !ok {
					t.Errorf("English label %q missing on %s", page.label, page.page)
				}
				dir := filepath.Join("..", "..", "output", "native")
				_ = os.MkdirAll(dir, 0755)
				file, err := os.Create(filepath.Join(dir, page.page+"-"+theme+"-en-1x.png"))
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

func TestNativeLanguageCatalogAndNoticeTemplates(t *testing.T) {
	t.Cleanup(func() { setUILanguage("zh") })
	setUILanguage("en")
	for _, text := range []string{"User's 中文回复", "/read C:/模型与预算.png", "provider/custom-model", "AGENTS.md"} {
		if tr(text) != text {
			t.Fatalf("arbitrary content was translated: %q", text)
		}
	}
	if got := localizeNotice("任务已暂停：预算不足。可输入 /continue 继续"); got != "Task paused: 预算不足. Enter /continue to resume" {
		t.Fatalf("pause status translation lost its original reason: %q", got)
	}
	if got := localizeNotice("后台命令 job-1：已完成"); got != "Background command job-1: completed" {
		t.Fatalf("job status translation failed: %q", got)
	}
	if got := localizeNotice("模型请求暂时失败，2 秒后重试（第 3 次）。"); !strings.Contains(got, "2 seconds (attempt 3)") {
		t.Fatalf("retry status translation failed: %q", got)
	}
	format := regexp.MustCompile(`%(?:\[[0-9]+\])?[-+# 0]*[0-9]*(?:\.[0-9]+)?[a-zA-Z%]`)
	for source, translated := range englishUI {
		if !slices.Equal(format.FindAllString(source, -1), format.FindAllString(translated, -1)) {
			t.Errorf("format arguments differ for %q: %q", source, translated)
		}
	}
	files, _ := filepath.Glob("*.go")
	for _, path := range files {
		if strings.HasSuffix(path, "_test.go") || path == "localization.go" {
			continue
		}
		file, err := parser.ParseFile(token.NewFileSet(), path, nil, 0)
		if err != nil {
			t.Fatal(err)
		}
		ast.Inspect(file, func(node ast.Node) bool {
			call, ok := node.(*ast.CallExpr)
			if !ok || len(call.Args) == 0 {
				return true
			}
			function, ok := call.Fun.(*ast.Ident)
			if !ok || function.Name != "tr" && function.Name != "trf" {
				return true
			}
			key, ok := call.Args[0].(*ast.BasicLit)
			if !ok || key.Kind != token.STRING {
				return true
			}
			value, _ := strconv.Unquote(key.Value)
			if strings.ContainsAny(value, "一二三四五六七八九十") || regexp.MustCompile(`[\p{Han}]`).MatchString(value) {
				if _, ok := englishUI[value]; !ok {
					t.Errorf("missing English translation in %s: %s", path, key.Value)
				}
			}
			return true
		})
	}
}
