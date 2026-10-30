package main

import (
	"minicode.desktop/internal/model"
	"testing"
)

func TestChildProgressIsVisibleAndFoldableDuringRun(t *testing.T) {
	r := newTestRig(t, "conversation")
	v := r.d.current()
	v.Items = []*model.FeedItem{{ID: "d", Kind: "tool", Name: "delegate", Pending: true, Args: map[string]any{"kind": "explore", "task": "inspect"}}}
	v.rebuildRows()
	r.d.runtimeEvent(v, model.Event{Type: "subagent_start", Data: map[string]any{"call_id": "d", "child_session_id": "child"}})
	r.d.runtimeEvent(v, model.Event{Type: "subagent_progress", Data: map[string]any{"call_id": "d", "child_session_id": "child", "child_call_id": "r", "name": "read", "phase": "tool", "status": "running", "child_event_seq": float64(3)}})
	r.ui.Frame()
	if !r.ui.HasText("子任务：read · 1 次工具调用") || r.ui.HasText("read · 运行中") {
		t.Fatal("collapsed progress missing or details exposed")
	}
	click(t, r.ui, "子助手 inspect")
	if !r.ui.HasText("read · 运行中") {
		t.Fatal("child details did not expand")
	}
	click(t, r.ui, "子助手 inspect")
	if r.ui.HasText("read · 运行中") {
		t.Fatal("child details did not collapse")
	}
}
