package main

import (
	"context"
	"encoding/json"
	"github.com/egoist/mygo/ui"
	"minicode.desktop/internal/bridge"
	"minicode.desktop/internal/model"
	"sync/atomic"
	"testing"
	"time"
)

type queueRequester struct{ calls chan string }

func (q *queueRequester) Request(_ context.Context, method string, _ any, target any) error {
	q.calls <- method
	if method == "listQueue" {
		data := []byte(`[{"id":"queued","mode":"steer","text":"direction"}]`)
		return json.Unmarshal(data, target)
	}
	return nil
}
func TestB6DesktopUsesServerQueueWithoutClientResubmission(t *testing.T) {
	r := newTestRig(t, "home")
	q := &queueRequester{calls: make(chan string, 16)}
	r.d.client = q
	r.d.messageQueue = true
	v := r.d.current()
	v.Busy = true
	v.SessionID = "session"
	r.d.send("direction", v)
	r.settle()
	if len(v.Queue) != 1 || v.Queue[0].ID != "queued" {
		t.Fatal("server queue was not displayed")
	}
	r.d.finish(v, bridge.Event{Event: "run_done", Result: []byte(`{"exit_reason":"completed"}`)})
	for len(q.calls) > 0 {
		if method := <-q.calls; method == "sendPrompt" {
			t.Fatal("client resubmitted server-owned message")
		}
	}
}

func TestD6ApprovalShortcutOnlyDecidesFirstPendingCard(t *testing.T) {
	r := newTestRig(t, "conversation")
	v := r.d.current()
	v.Items = []*model.FeedItem{
		{ID: "first", Kind: "approval", Name: "write", ApprovalID: "first", Args: map[string]any{}},
		{ID: "second", Kind: "approval", Name: "write", ApprovalID: "second", Args: map[string]any{}},
	}
	v.rebuildRows()
	r.ui.Frame()
	r.ui.Key(0, ui.KeyY)
	if approvalShortcutTarget(v.Items) != v.Items[0] {
		t.Fatal("shortcut must target the first card regardless of focus")
	}
	v.Items[0].Deciding = true
	if approvalShortcutTarget(v.Items) != nil {
		t.Fatal("a deciding card must block shortcut fallthrough")
	}
	if v.Items[1].Deciding || v.Items[1].ApprovalID == "" {
		t.Fatal("one key decided two cards")
	}
}

type blockingRequests struct {
	active, peak atomic.Int32
	release      chan struct{}
}

func (b *blockingRequests) Request(ctx context.Context, _ string, _ any, _ any) error {
	n := b.active.Add(1)
	defer b.active.Add(-1)
	for {
		old := b.peak.Load()
		if n <= old || b.peak.CompareAndSwap(old, n) {
			break
		}
	}
	select {
	case <-b.release:
		return nil
	case <-ctx.Done():
		return ctx.Err()
	}
}
func TestD7DesktopBoundsRequestGoroutines(t *testing.T) {
	r := newTestRig(t, "home")
	b := &blockingRequests{release: make(chan struct{})}
	r.d.client = b
	for range 80 {
		r.d.request(r.d.current(), "getState", nil, nil, func(error) {})
	}
	time.Sleep(30 * time.Millisecond)
	if n := b.peak.Load(); n > 16 {
		close(b.release)
		t.Fatalf("concurrent requests=%d", n)
	}
	close(b.release)
	r.settle()
}

func TestTU6TasksRemainVisibleInConversation(t *testing.T) {
	r := newTestRig(t, "conversation")
	r.d.preferences.RightCollapsed = true
	r.d.current().Tasks = []model.Task{{Title: "Persistent task", Status: "running"}}
	r.ui.Frame()
	if _, ok := r.ui.Find("Persistent task"); !ok {
		t.Fatal("task only exists in the hidden work tab")
	}
}

type prioritizedRequests struct {
	blockingRequests
	controls chan struct{}
}

func (b *prioritizedRequests) Request(ctx context.Context, method string, params any, target any) error {
	if method == "cancelTurn" {
		b.controls <- struct{}{}
		return nil
	}
	return b.blockingRequests.Request(ctx, method, params, target)
}
func TestD7CancellationBypassesSaturatedNormalRequests(t *testing.T) {
	r := newTestRig(t, "home")
	b := &prioritizedRequests{blockingRequests: blockingRequests{release: make(chan struct{})}, controls: make(chan struct{}, 1)}
	r.d.client = b
	defer close(b.release)
	for range 16 {
		r.d.request(r.d.current(), "getState", nil, nil, func(error) {})
	}
	r.d.request(r.d.current(), "cancelTurn", nil, nil, func(error) {})
	select {
	case <-b.controls:
	case <-time.After(100 * time.Millisecond):
		t.Fatal("cancellation queued behind blocked normal requests")
	}
}
