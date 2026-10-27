package main

import "testing"

func TestAnthropicServiceCanEnableEffort(t *testing.T) {
	r := newTestRig(t, "conversation")
	r.d.openSettings("model")
	r.settle()
	click(t, r.ui, "编辑 CommandCode")
	s := r.d.settings
	s.Service.APIStyle = "anthropic"
	s.Service.Models[0].SupportsEffort = false
	s.OpenModels[0] = true
	r.ui.Frame()
	r.ui.Scroll(900, 700, 0, 800)
	click(t, r.ui, "支持思考强度")
	if !s.Service.Models[0].SupportsEffort {
		t.Fatal("Anthropic effort checkbox remained disabled")
	}
}
