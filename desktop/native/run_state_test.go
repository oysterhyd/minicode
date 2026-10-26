package main

import (
	"encoding/json"
	"minicode.desktop/internal/bridge"
	"minicode.desktop/internal/model"
	"testing"
)

func TestOldRunTerminalDoesNotFinishNewRun(t *testing.T) {
	r := newTestRig(t, "home")
	v := r.d.current()
	v.Busy = true
	v.State.RunID = "new"
	v.SessionID = "session"
	for _, kind := range []string{"run_done", "run_error"} {
		var event bridge.Event
		raw, _ := json.Marshal(map[string]any{"event": kind, "clientKey": v.Key, "sessionId": "session", "runId": "old", "error": "old failure"})
		_ = json.Unmarshal(raw, &event)
		r.d.handleEvent(event)
		if !v.Busy || v.Error != "" {
			t.Fatal("old terminal finished current run")
		}
	}
	var event bridge.Event
	raw, _ := json.Marshal(map[string]any{"event": "run_done", "clientKey": v.Key, "sessionId": "session", "runId": "new"})
	_ = json.Unmarshal(raw, &event)
	r.d.handleEvent(event)
	if v.Busy {
		t.Fatal("current terminal was ignored")
	}
	v.Busy = true
	r.d.handleEvent(bridge.Event{Event: "run_done", ClientKey: v.Key, SessionID: "session"})
	if v.Busy {
		t.Fatal("legacy terminal was ignored")
	}
}

func TestJoiningRunningSessionTracksBusyAndRepairsLoadedTools(t *testing.T) {
	r := newTestRig(t, "home")
	f := r.d.client.(*fixtureClient)
	f.State.Running = true
	f.State.RunID = "joined"
	f.Sessions = []model.Session{{ID: "live", Workspace: r.d.current().Workspace, Model: "fake"}}
	r.d.openSession(f.Sessions[0])
	r.settle()
	v := r.d.current()
	if !v.Busy || v.State.RunID != "joined" {
		t.Fatal("joined a running session as idle")
	}
	v.Items = []*model.FeedItem{{ID: "call", Kind: "tool", Name: "read", Interrupted: true}}
	v.rebuildRows()
	r.d.runtimeEvent(v, model.Event{Type: "tool_call_result", Data: map[string]any{"call_id": "call", "success": true, "output_preview": "result"}})
	if v.Items[0].Interrupted {
		t.Fatal("live result retained stale interrupted marker")
	}
}
