package main

import (
	"encoding/json"
	"testing"

	"minicode.desktop/internal/bridge"
	"minicode.desktop/internal/model"
)

func TestApprovalDecisionOnlyUpdatesItsCall(t *testing.T) {
	r := newTestRig(t, "home")
	v := r.d.current()
	for _, call := range []string{"first", "second"} {
		r.d.runtimeEvent(v, model.Event{Type: "approval_request", Data: map[string]any{"call_id": call}})
	}
	// Requests may interleave; each card must carry its own call identity.
	for _, call := range []string{"second", "first"} {
		request, _ := json.Marshal(map[string]any{"call_id": call, "tool_name": "write"})
		r.d.handleEvent(bridge.Event{Event: "approval", ClientKey: v.Key, ApprovalID: call, Request: request})
	}
	r.d.runtimeEvent(v, model.Event{Type: "approval_decision", Data: map[string]any{"call_id": "first", "granted": true}})
	if v.Items[0].ApprovalID != "second" || v.Items[0].Granted || v.Items[1].ApprovalID != "" || !v.Items[1].Granted {
		t.Fatal("one decision changed another approval card")
	}
	r.d.runtimeEvent(v, model.Event{Type: "approval_decision", Data: map[string]any{"call_id": "unknown", "granted": false}})
	if v.Items[0].ApprovalID != "second" {
		t.Fatal("unknown call cleared a pending card")
	}
}

func TestLegacyApprovalUsesPrecedingRequestCall(t *testing.T) {
	r := newTestRig(t, "home")
	v := r.d.current()
	for _, call := range []string{"first", "second"} {
		r.d.runtimeEvent(v, model.Event{Type: "approval_request", Data: map[string]any{"call_id": call}})
		r.d.handleEvent(bridge.Event{Event: "approval", ClientKey: v.Key, ApprovalID: call, Request: []byte(`{"tool_name":"write"}`)})
	}
	r.d.runtimeEvent(v, model.Event{Type: "approval_decision", Data: map[string]any{"call_id": "first", "granted": false}})
	if v.Items[0].ApprovalID != "" || v.Items[1].ApprovalID != "second" {
		t.Fatal("legacy decision cleared unrelated approval")
	}
}
