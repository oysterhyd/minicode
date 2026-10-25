package main

import (
	"testing"

	"minicode.desktop/internal/bridge"
)

// SEC1: the workspace trust request is a trust question, not a tool approval
// that could be "always allowed" for the conversation.
func TestWorkspaceTrustApprovalCard(t *testing.T) {
	r := newTestRig(t, "home")
	v := r.d.current()
	v.Busy = true
	r.d.handleEvent(bridge.Event{Event: "approval", ClientKey: v.Key, ApprovalID: "trust-1", Request: []byte(`{"tool_name":"workspace_trust","arguments":{"workspace":"/repo","plugins":[{"name":"evil","servers":[{"name":"evil","command":"node","args":["server.js"]}]}]},"summary":"信任此工作区的插件？"}`)})
	r.ui.Frame()
	if _, ok := r.ui.Find("信任并启动"); !ok {
		t.Fatal("trust card has no trust button")
	}
	if _, ok := r.ui.Find("本会话始终允许 workspace_trust"); ok {
		t.Fatal("workspace trust must not offer always-allow")
	}
}
