package main

import "testing"

func TestLocalExtensionFilterAndActivation(t *testing.T) {
	r := newTestRig(t, "settings-skills")
	e := r.d.settings.Extensions
	if len(visibleExtensions(e, "skill")) != 2 {
		t.Fatal("missing global or project resources")
	}
	e.Scope = "project"
	if rows := visibleExtensions(e, "skill"); len(rows) != 1 || rows[0].ID != "skill-b" {
		t.Fatal("scope filtering failed")
	}
	e.Scope, e.Query = "all", "AGENTS"
	if rows := visibleExtensions(e, "skill"); len(rows) != 1 || rows[0].ID != "skill-a" {
		t.Fatal("source search failed")
	}
	e.Query = ""
	r.ui.Frame()
	click(t, r.ui, "启用本地资源 project-review")
	r.settle()
	if e.Pending != "" || e.Error != "" {
		t.Fatalf("activation did not complete: %s", e.Error)
	}
	for _, item := range e.Catalog.Entries {
		if item.ID == "skill-b" && !item.Enabled {
			t.Fatal("activation did not reach bridge")
		}
	}
	e.Catalog.Entries = append(e.Catalog.Entries, localExtension{ID: "managed", Kind: "skill", ParentID: "plugin-a", Supported: true})
	if len(visibleExtensions(e, "skill")) != 2 {
		t.Fatal("plugin-managed child exposed as independent toggle")
	}
}

func TestGlobalInstructionsEditorSaveAndRestore(t *testing.T) {
	r := newTestRig(t, "settings-instructions")
	a := r.d.settings.Instructions
	original := a.Saved
	a.Content = "Use English.\nRun focused checks."
	r.ui.Frame()
	click(t, r.ui, "还原已保存内容")
	if a.Content != original {
		t.Fatal("restore lost saved instructions")
	}
	a.Content = "Use English.\nRun focused checks."
	r.ui.Frame()
	click(t, r.ui, "保存指令")
	r.settle()
	if a.Saving || a.Error != "" || a.Saved != a.Content {
		t.Fatalf("save failed: %s", a.Error)
	}
	if r.d.client.(*fixtureClient).Instructions.Content != a.Content {
		t.Fatal("save did not reach bridge")
	}
}
