package main

import (
	"minicode.desktop/internal/model"
	"testing"
)

func TestScopedApprovalDisplaysTheConcreteRuleAndProjectChoice(t *testing.T) {
	r := newTestRig(t, "conversation")
	v := r.d.current()
	v.Items = []*model.FeedItem{{ID: "approval", Kind: "approval", Name: "bash", Args: map[string]any{"command": "npm test"}, ApprovalID: "a", GrantRule: "bash(npm test:*)"}}
	v.rebuildRows()
	r.ui.Frame()
	if _, ok := r.ui.Find("本会话允许：bash(npm test:*)"); !ok {
		t.Fatal("scoped session choice missing")
	}
	if _, ok := r.ui.Find("写入项目规则"); !ok {
		t.Fatal("project rule choice missing")
	}
	if _, ok := r.ui.Find("批准执行"); !ok {
		t.Fatal("once choice missing")
	}
}
