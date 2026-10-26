package main

import (
	"context"
	"errors"
	"testing"
	"time"

	"minicode.desktop/internal/bridge"
	"minicode.desktop/internal/model"
)

func TestGlobalFatalBridgeErrorReportsOnceAndPreservesViewErrors(t *testing.T) {
	r := newTestRig(t, "home")
	v := r.d.current()
	v.Busy, v.AwaitingRun, v.State.Running = true, true, true
	v.Error = "existing conversation error"
	v.StartedAt = time.Now()
	tool := &model.FeedItem{ID: "pending", Kind: "tool", Pending: true}
	v.append(tool)
	other := newConversation("other", v.Workspace, initialState())
	other.Busy = true
	other.Queue = []queuedPrompt{{Text: "keep queue"}}
	idle := newConversation("idle", v.Workspace, initialState())
	idle.State.Phase = "idle"
	r.d.views[other.Key], r.d.views[idle.Key] = other, idle
	notifications := 0
	r.d.preferences.Notifications = true
	r.d.notify = func(string, string, string) { notifications++ }
	event := bridge.Event{Event: "bridge_error", Fatal: true, Error: "Agent bridge output closed"}
	r.d.handleEvent(event)
	if notifications != 1 || len(r.d.toasts) != 1 {
		t.Fatalf("global failure reported repeatedly: notifications=%d toasts=%d", notifications, len(r.d.toasts))
	}
	if v.Busy || v.AwaitingRun || v.State.Running || other.Busy || tool.Pending || !tool.Interrupted || len(other.Queue) != 1 {
		t.Fatal("global failure left active controls or lost queued input")
	}
	r.ui.Frame()
	if _, found := r.ui.Find(event.Error); !found {
		t.Fatal("global connection status is not visible")
	}
	if v.Error != "existing conversation error" || other.Error != "" || idle.Error != "" || idle.State.Phase != "idle" {
		t.Fatal("global error polluted conversation state")
	}
	count := len(v.Items)
	r.d.handleEvent(event)
	r.d.handleEvent(bridge.Event{Event: "run_started", ClientKey: v.Key, RunID: "late"})
	if notifications != 1 || len(r.d.toasts) != 1 || len(v.Items) != count || v.Busy {
		t.Fatal("duplicate failure or late events reopened a failed connection")
	}
	r.d.send("keep draft", v)
	if v.Busy || v.Draft != "keep draft" || len(v.Items) != count {
		t.Fatal("send on a failed connection discarded input or started a run")
	}
	v.Draft = ""
	r.d.slash("/continue", v)
	if v.Busy || v.Draft != "/continue" {
		t.Fatal("failed connection discarded a remote command")
	}
}

func TestExplicitHandshakeClearsFailureButLateHandshakeCannot(t *testing.T) {
	r := newTestRig(t, "home")
	event := bridge.Event{Event: "bridge_error", Fatal: true, Error: "connection closed"}
	r.d.handleEvent(event)
	r.d.initialize()
	r.settle()
	if r.d.bridgeFailed || r.d.bridgeError != "" {
		t.Fatal("successful explicit handshake retained failed connection state")
	}
	r.d.initialize()
	r.d.handleEvent(event)
	r.settle()
	if !r.d.bridgeFailed {
		t.Fatal("late handshake erased a newer connection failure")
	}
}

func TestGlobalNonfatalBridgeErrorDoesNotFinishAnyRun(t *testing.T) {
	r := newTestRig(t, "home")
	v := r.d.current()
	v.Busy = true
	event := bridge.Event{Event: "bridge_error", Error: "Error: invalid request; child has not exited"}
	r.d.handleEvent(event)
	r.d.handleEvent(event)
	if !v.Busy || v.Error != "" || len(r.d.toasts) != 1 {
		t.Fatal("nonfatal global protocol error finished or polluted a conversation")
	}
}

func TestScopedBridgeErrorOnlyUpdatesItsView(t *testing.T) {
	r := newTestRig(t, "home")
	v := r.d.current()
	v.Busy = true
	other := newConversation("other", v.Workspace, initialState())
	other.Busy = true
	r.d.views[other.Key] = other
	r.d.handleEvent(bridge.Event{Event: "bridge_error", ClientKey: v.Key, Error: "Error: invalid request"})
	if !v.Busy || !other.Busy || v.Error != "Error: invalid request" || other.Error != "" || len(r.d.toasts) != 0 {
		t.Fatal("scoped error affected another view or finished a run")
	}
	other.SessionID = "other-session"
	r.d.handleEvent(bridge.Event{Event: "bridge_error", SessionID: other.SessionID, Fatal: true, Error: "session failed"})
	if !v.Busy || other.Busy || other.Error != "session failed" || r.d.bridgeFailed {
		t.Fatal("session-scoped fatal error closed the global connection")
	}
}

func TestFatalBridgeErrorRejectsLateSendReplyWithoutRollingBackInput(t *testing.T) {
	r := newTestRig(t, "home")
	base := r.d.client.(*fixtureClient)
	entered, release := make(chan struct{}), make(chan struct{})
	t.Cleanup(func() {
		select {
		case <-release:
		default:
			close(release)
		}
	})
	r.d.client = auditRequester(func(ctx context.Context, method string, params, target any) error {
		if method == "sendPrompt" {
			close(entered)
			<-release
			return errors.New("connection closed")
		}
		return base.Request(ctx, method, params, target)
	})
	v := r.d.current()
	r.d.send("pending acceptance", v)
	<-entered
	r.d.handleEvent(bridge.Event{Event: "bridge_error", Fatal: true, Error: "connection closed"})
	close(release)
	r.settle()
	if v.Busy || v.Draft != "" || v.Error != "" || len(v.Items) == 0 || v.Items[0].Text != "pending acceptance" || len(r.d.toasts) != 1 {
		t.Fatal("late send reply rolled back history or repeated the global error")
	}
}
