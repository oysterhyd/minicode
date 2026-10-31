package main

import (
	"context"
	"github.com/egoist/mygo/ui"
	"minicode.desktop/internal/model"
	"sync/atomic"
	"testing"
	"time"
)

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
