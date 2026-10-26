package main

import (
	"context"
	"encoding/json"
	"minicode.desktop/internal/model"
	"testing"
)

type historyClient struct {
	*fixtureClient
	pages   int
	started chan struct{}
	unblock chan struct{}
}

func (c *historyClient) Request(ctx context.Context, method string, params any, target any) error {
	if method != "getSessionPage" {
		return c.fixtureClient.Request(ctx, method, params, target)
	}
	c.pages++
	if c.unblock != nil {
		close(c.started)
		<-c.unblock
	}
	return json.Unmarshal([]byte(`{"summary":{"session_id":"paged"},"messages":[{"role":"user","content":[{"type":"text","text":"earliest"}]}],"events":[],"messageOffset":0,"cursor":{"messages":null,"events":null}}`), target)
}
func TestHistoryLoadsEarlierPagesOnDemand(t *testing.T) {
	r := newTestRig(t, "home")
	f := r.d.client.(*fixtureClient)
	f.Sessions = []model.Session{{ID: "paged", Workspace: r.d.current().Workspace, Model: "fake"}}
	if err := json.Unmarshal([]byte(`{"messages":[{"role":"user","content":[{"type":"text","text":"recent"}]}],"events":[],"messageOffset":1,"cursor":{"messages":1,"events":null}}`), &f.Detail); err != nil {
		t.Fatal(err)
	}
	client := &historyClient{fixtureClient: f}
	r.d.client = client
	r.d.openSession(f.Sessions[0])
	r.settle()
	if client.pages != 0 {
		t.Fatal("older history loaded eagerly")
	}
	click(t, r.ui, "加载更早历史")
	r.settle()
	v := r.d.current()
	if client.pages != 1 || len(v.Items) != 2 || v.Items[0].Text != "earliest" || v.Items[1].Text != "recent" {
		t.Fatalf("history was lost or duplicated: %+v", v.Items)
	}
	if v.Items[0].ID == v.Items[1].ID {
		t.Fatal("paged history IDs collide")
	}
}

func TestClearWhileHistoryPageLoadsDoesNotRestoreOrPanic(t *testing.T) {
	r := newTestRig(t, "home")
	f := r.d.client.(*fixtureClient)
	f.Sessions = []model.Session{{ID: "paged", Workspace: r.d.current().Workspace, Model: "fake"}}
	_ = json.Unmarshal([]byte(`{"messages":[{"role":"user","content":[{"type":"text","text":"recent"}]}],"messageOffset":1,"cursor":{"messages":1,"events":null}}`), &f.Detail)
	client := &historyClient{fixtureClient: f, started: make(chan struct{}), unblock: make(chan struct{})}
	r.d.client = client
	r.d.openSession(f.Sessions[0])
	r.settle()
	v := r.d.current()
	r.d.loadEarlierHistory(v)
	<-client.started
	r.d.slash("/clear", v)
	close(client.unblock)
	r.settle()
	if len(v.Items) != 0 || v.History != nil {
		t.Fatal("cleared history was restored by a late response")
	}
}

func TestHistoryPagePreservesLiveUpdatesToLoadedTools(t *testing.T) {
	r := newTestRig(t, "home")
	f := r.d.client.(*fixtureClient)
	f.Sessions = []model.Session{{ID: "paged", Workspace: r.d.current().Workspace, Model: "fake"}}
	_ = json.Unmarshal([]byte(`{"messages":[{"role":"assistant","content":[{"type":"tool_use","id":"call","name":"read","input":{"path":"live.txt"}}]}],"messageOffset":1,"cursor":{"messages":1,"events":null}}`), &f.Detail)
	client := &historyClient{fixtureClient: f, started: make(chan struct{}), unblock: make(chan struct{})}
	r.d.client = client
	r.d.openSession(f.Sessions[0])
	r.settle()
	v := r.d.current()
	r.d.loadEarlierHistory(v)
	<-client.started
	r.d.runtimeEvent(v, model.Event{Type: "tool_call_result", Data: map[string]any{"call_id": "call", "success": true, "output_preview": "LIVE RESULT"}})
	close(client.unblock)
	r.settle()
	if len(v.Items) != 2 || !v.Items[1].Success || v.Items[1].Output != "LIVE RESULT" {
		t.Fatal("pagination overwrote a live tool result")
	}
}
