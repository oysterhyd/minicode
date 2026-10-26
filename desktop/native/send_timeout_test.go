package main

import (
	"context"
	"encoding/json"
	"minicode.desktop/internal/bridge"
	"os"
	"testing"
	"time"
)

type timeoutSendClient struct {
	*fixtureClient
	running bool
	wait    bool
	reads   int
}

func (c *timeoutSendClient) Request(ctx context.Context, method string, p any, target any) error {
	if method == "sendPrompt" {
		if c.wait {
			<-ctx.Done()
		}
		return context.DeadlineExceeded
	}
	if method == "getState" {
		c.reads++
		state := c.State
		state.SessionID = "accepted-session"
		state.RunID = "accepted-run"
		state.Running = c.running
		if !c.running {
			state.RunID = ""
			state.SessionID = ""
		}
		raw, _ := json.Marshal(state)
		return json.Unmarshal(raw, target)
	}
	return c.fixtureClient.Request(ctx, method, p, target)
}
func TestSendTimeoutReconcilesAcceptedRun(t *testing.T) {
	r := newTestRig(t, "home")
	client := &timeoutSendClient{fixtureClient: r.d.client.(*fixtureClient), running: true}
	r.d.client = client
	v := r.d.current()
	r.d.send("accepted prompt", v)
	r.settle()
	if !v.Busy || client.reads == 0 || v.Draft != "" {
		t.Fatal("timeout rolled back an accepted run")
	}
	found := false
	for _, item := range v.Items {
		if item.Kind == "user" && item.Text == "accepted prompt" {
			found = true
		}
	}
	if !found || v.State.RunID != "accepted-run" {
		t.Fatal("accepted prompt or identity was lost")
	}
	r.d.handleEvent(bridge.Event{Event: "run_done", ClientKey: v.Key, SessionID: v.SessionID, RunID: "accepted-run"})
	if v.Busy {
		t.Fatal("confirmed run never finished")
	}
}
func TestSendTimeoutRollsBackOnlyWhenStateConfirmsNoRun(t *testing.T) {
	r := newTestRig(t, "home")
	client := &timeoutSendClient{fixtureClient: r.d.client.(*fixtureClient)}
	r.d.client = client
	v := r.d.current()
	// A state with a different completed run still confirms an idle bridge.
	r.d.send("not accepted", v)
	r.settle()
	if v.Busy || client.reads == 0 || v.Draft != "not accepted" {
		t.Fatal("idle reconciliation failed")
	}
	for _, item := range v.Items {
		if item.Kind == "user" && item.Text == "not accepted" {
			t.Fatal("rejected prompt kept a duplicate bubble")
		}
	}
}

func TestSendPromptDeadlineReconcilesAt35Seconds(t *testing.T) {
	if os.Getenv("MINICODE_SLOW_TESTS") != "1" {
		t.Skip("35 second real request deadline; MINICODE_SLOW_TESTS=1")
	}
	r := newTestRig(t, "home")
	client := &timeoutSendClient{fixtureClient: r.d.client.(*fixtureClient), running: true, wait: true}
	r.d.client = client
	v := r.d.current()
	start := time.Now()
	r.d.send("slow accepted prompt", v)
	for v.AwaitingRun && time.Since(start) < 38*time.Second {
		r.settle()
	}
	if time.Since(start) < 35*time.Second || v.AwaitingRun || !v.Busy || v.Draft != "" {
		t.Fatal("real 35s timeout produced false idle")
	}
	r.d.handleEvent(bridge.Event{Event: "run_done", ClientKey: v.Key, SessionID: v.SessionID, RunID: "accepted-run"})
	if v.Busy {
		t.Fatal("accepted run did not settle")
	}
}
