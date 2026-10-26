package main

import (
	"encoding/json"
	"minicode.desktop/internal/bridge"
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
