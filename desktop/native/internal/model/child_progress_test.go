package model

import "testing"

func TestChildProgressReplayAndLiveUseExactCallAndMonotonicStatus(t *testing.T) {
	args := map[string]any{"kind": "explore", "task": "same"}
	items := []*FeedItem{{ID: "d", Kind: "tool", Name: "delegate", Args: args, Pending: true}, {ID: "other", Kind: "tool", Name: "delegate", Args: args, Pending: true}}
	events := []Event{
		{Type: "subagent_start", Data: map[string]any{"call_id": "d", "child_session_id": "child", "kind": "explore", "task": "same"}},
		{Type: "subagent_progress", Data: map[string]any{"call_id": "d", "child_session_id": "child", "child_call_id": "r", "child_event_seq": float64(8), "name": "read", "phase": "tool", "status": "completed"}},
		{Type: "subagent_progress", Data: map[string]any{"call_id": "d", "child_session_id": "child", "child_call_id": "r", "child_event_seq": float64(6), "name": "read", "phase": "tool", "status": "running"}},
		{Type: "subagent_result", Data: map[string]any{"call_id": "d", "child_session_id": "child", "exit_reason": "completed"}},
	}
	for _, event := range events {
		ApplyChildEvent(items, event)
	}
	if len(items[0].ChildProgress) != 1 || items[0].ChildProgress[0].Status != "completed" || items[0].ChildStatus != "completed" || items[1].ChildSessionID != "" {
		t.Fatalf("incorrect live progress: %+v / %+v", items[0], items[1])
	}
	history := History(Detail{Messages: []Message{{Role: "assistant", Content: []map[string]any{{"type": "tool_use", "id": "d", "name": "delegate", "input": args}, {"type": "tool_use", "id": "other", "name": "delegate", "input": args}}}}, Events: events})
	if len(history[0].ChildProgress) != 1 || history[0].ChildProgress[0].Status != "completed" || history[0].ChildStatus != "completed" || history[1].ChildSessionID != "" {
		t.Fatalf("incorrect history: %+v", history)
	}
}
