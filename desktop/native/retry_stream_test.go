package main

import (
	"strings"
	"testing"

	"minicode.desktop/internal/bridge"
	"minicode.desktop/internal/model"
)

func TestProviderRetryDiscardsOnlyCurrentStream(t *testing.T) {
	d := newDesktop(func(f func()) { f() })
	v := d.current()
	v.SessionID = "retry-session"
	v.Busy = true
	previous := &model.FeedItem{ID: "previous", Kind: "assistant", Text: "older answer"}
	v.append(previous)
	d.runtimeEvent(v, model.Event{Seq: 1, Type: "round_start"})
	d.handleEvent(bridge.Event{Event: "reasoning_delta", ClientKey: v.Key, SessionID: v.SessionID, Text: "discard reasoning"})
	d.handleEvent(bridge.Event{Event: "text_delta", ClientKey: v.Key, SessionID: v.SessionID, Text: "discard partial"})
	d.runtimeEvent(v, model.Event{Seq: 2, Type: "provider_retry", Data: map[string]any{"reset_output": true, "delay_s": 0, "next_attempt": 2}})
	if v.stream != nil || v.reasoning != nil {
		t.Fatal("retry retained current stream pointers")
	}
	for _, item := range v.Items {
		if strings.Contains(item.Text, "discard") {
			t.Fatalf("retry retained partial output: %+v", item)
		}
	}
	if v.Items[0] != previous {
		t.Fatal("retry removed a completed answer")
	}
	d.handleEvent(bridge.Event{Event: "text_delta", ClientKey: v.Key, SessionID: v.SessionID, Text: "new answer"})
	if v.stream == nil || v.stream.Text != "new answer" {
		t.Fatal("retry output was appended to discarded text")
	}
}
