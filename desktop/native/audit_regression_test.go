package main

import (
	"context"
	"encoding/json"
	"strings"
	"testing"
	"time"

	"minicode.desktop/internal/bridge"
	"minicode.desktop/internal/model"
)

type auditRequester func(context.Context, string, any, any) error

func (f auditRequester) Request(ctx context.Context, method string, params, target any) error {
	return f(ctx, method, params, target)
}

func TestContinueCompletionBeforeReplyDoesNotLeaveTheViewBusy(t *testing.T) {
	queue := make(chan func(), 30)
	d := newDesktop(func(fn func()) { queue <- fn })
	t.Cleanup(func() { d.closed.Store(true) })
	v := d.current()
	v.SessionID, v.Workspace = "audit", t.TempDir()
	d.client = auditRequester(func(_ context.Context, method string, _ any, target any) error {
		if method == "runSlash" {
			d.schedule(func() {
				d.finish(v, bridge.Event{Event: "run_done", Result: json.RawMessage(`{"exit_reason":"completed"}`)})
			})
			return json.Unmarshal([]byte(`{"action":"continue"}`), target)
		}
		if target != nil {
			return json.Unmarshal([]byte(`null`), target)
		}
		return nil
	})
	d.slash("/continue", v)
	if !v.Busy {
		t.Fatal("continue was not marked busy before dispatch")
	}
	for i := 0; i < 2; i++ {
		select {
		case fn := <-queue:
			fn()
		case <-time.After(time.Second):
			t.Fatal("missing completion or reply")
		}
	}
	if v.Busy {
		t.Fatal("late slash reply overwrote completion")
	}
}

func TestStopIntentAndFatalBridgeErrorsPreserveQueuedInput(t *testing.T) {
	d := newDesktop(func(fn func()) { fn() })
	v := d.current()
	v.Busy, v.CancelPending = true, true
	v.Queue = []queuedPrompt{{Text: "keep queued"}}
	d.finish(v, bridge.Event{Event: "run_done", Result: json.RawMessage(`{"exit_reason":"completed"}`)})
	if v.Busy || len(v.Queue) != 1 {
		t.Fatal("stop intent drained the queue")
	}
	v.Busy = true
	item := &model.FeedItem{ID: "tool", Kind: "tool", Pending: true}
	v.append(item)
	previousError := v.Error
	d.handleEvent(bridge.Event{Event: "bridge_error", Fatal: true, Error: "Agent bridge output closed"})
	if v.Busy || item.Pending || !item.Interrupted || v.Error != previousError || !strings.Contains(d.bridgeError, "output closed") {
		t.Fatal("fatal bridge error left active controls")
	}
	if len(v.Queue) != 1 {
		t.Fatal("fatal error lost the queued prompt")
	}
}

func TestSubmitWithoutWorkspaceKeepsTheDraft(t *testing.T) {
	d := newDesktop(func(fn func()) { fn() })
	v := d.current()
	v.Draft = "Please inspect the repository"
	d.submit()
	if v.Draft != "Please inspect the repository" || v.Error == "" {
		t.Fatal("missing workspace discarded user input")
	}
}
