package main

import (
	"image/png"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/egoist/mygo/ui"
	"minicode.desktop/internal/model"
)

func TestT4BatchEditApprovalPreviewIncludesAllOperationsAndRefreshes(t *testing.T) {
	d := newDesktop(func(fn func()) { fn() })
	edits := []any{map[string]any{"old_text": "before-one", "new_text": "after-one"}, map[string]any{"old_text": "before-two", "new_text": "after-two"}}
	item := &model.FeedItem{ID: "batch-edit", Name: "edit", Args: map[string]any{"path": "batch.txt", "edits": edits}}
	var text strings.Builder
	for _, line := range d.itemDiff(item) {
		text.WriteString(line.Text)
		text.WriteByte('\n')
	}
	for _, want := range []string{"before-one", "after-one", "before-two", "after-two"} {
		if !strings.Contains(text.String(), want) {
			t.Fatalf("missing %q: %s", want, text.String())
		}
	}
	edits[1].(map[string]any)["new_text"] = "updated-two"
	text.Reset()
	for _, line := range d.itemDiff(item) {
		text.WriteString(line.Text)
		text.WriteByte('\n')
	}
	if !strings.Contains(text.String(), "updated-two") {
		t.Fatalf("stale batch preview: %s", text.String())
	}
}

func TestWritePreviewUsesActualDiffAndInvalidatesOnResult(t *testing.T) {
	d := newDesktop(func(fn func()) { fn() })
	item := &model.FeedItem{ID: "file-write", Name: "write", Success: true, Args: map[string]any{"content": "new value"}, Output: "Applied change to page.css\n--- a/page.css\n+++ b/page.css\n@@ -42 +42 @@\n-old value\n+new value\n"}
	lines := d.itemDiff(item)
	if len(lines) != 3 || lines[1].Type != "remove" || lines[1].OldLine != 42 || lines[2].Type != "add" || lines[2].NewLine != 42 {
		t.Fatalf("actual file diff was replaced by an argument preview: %+v", lines)
	}
	item.Output = "@@ -8 +8 @@\n-old value\n+updated result\n"
	lines = d.itemDiff(item)
	if len(lines) != 3 || lines[2].Text != "updated result" || lines[2].NewLine != 8 {
		t.Fatalf("completed output reused a stale diff: %+v", lines)
	}
}

func TestWritePreviewHasOneConsistentCodeLayout(t *testing.T) {
	for _, dark := range []bool{false, true} {
		d := newDesktop(func(fn func()) { fn() })
		item := &model.FeedItem{ID: "css-write", Name: "write", Kind: "tool", Success: true,
			Args:   map[string]any{"path": "page.css", "content": ".bar {\n  color: red;\n}"},
			Output: "Applied change to page.css (+3 -0)\n--- a/page.css\n+++ b/page.css\n@@ -0,0 +1,3 @@\n+.bar {\n+  color: red;\n+}\n"}
		tt := ui.NewTester(func(c *ui.Context) { d.tool(c, colors(dark), item, true) }, 900, 600)
		first, ok := tt.Find(".bar {")
		if !ok {
			t.Fatal("file code line missing")
		}
		second, ok := tt.Find("  color: red;")
		if !ok || second.Y <= first.Y || second.Y-first.Y > first.H+4.5 {
			t.Fatalf("code lines gained paragraph gaps: first=%+v second=%+v", first, second)
		}
		if tt.HasText(item.Output) {
			t.Fatal("the same write output was rendered twice in different formats")
		}
		dir := filepath.Join("..", "..", "output", "native")
		if err := os.MkdirAll(dir, 0755); err != nil {
			t.Fatal(err)
		}
		name := "write-preview-light.png"
		if dark {
			name = "write-preview-dark.png"
		}
		file, err := os.Create(filepath.Join(dir, name))
		if err != nil {
			t.Fatal(err)
		}
		if err := png.Encode(file, tt.Image()); err != nil {
			file.Close()
			t.Fatal(err)
		}
		file.Close()
	}
}

func TestMCPDiscoveryDoesNotCreateChatWarnings(t *testing.T) {
	d := newDesktop(func(fn func()) { fn() })
	v := d.current()
	v.refreshPending = true
	d.runtimeEvent(v, model.Event{Type: "mcp_discovery", Data: map[string]any{"server": "offline", "error": "MCP connection failed"}})
	if len(v.Items) != 0 {
		t.Fatal("startup connection diagnostics were inserted into chat")
	}
	d.runtimeEvent(v, model.Event{Type: "tool_call_start", Data: map[string]any{"call_id": "echo", "name": "mcp__online__echo", "arguments": map[string]any{}}})
	d.runtimeEvent(v, model.Event{Type: "tool_call_result", Data: map[string]any{"call_id": "echo", "success": false, "output_detail": "connection lost"}})
	if len(v.Items) != 1 || v.Items[0].Success || v.Items[0].Output != "connection lost" {
		t.Fatal("an actual MCP call failure was hidden")
	}
}
