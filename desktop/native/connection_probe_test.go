package main

import "testing"

func TestConnectionProbeAndModelDiscoveryAreSeparate(t *testing.T) {
	r := newTestRig(t, "conversation")
	r.d.openSettings("model")
	r.settle()
	click(t, r.ui, "编辑 CommandCode")
	click(t, r.ui, "测试连接")
	r.settle()
	if r.d.settings.Status != "连接成功" || len(r.d.settings.Found) != 0 {
		t.Fatalf("connection probe unexpectedly fetched a list: %+v", r.d.settings)
	}
	click(t, r.ui, "获取模型列表")
	r.settle()
	if len(r.d.settings.Found) != 1 {
		t.Fatal("model discovery did not return its list")
	}
}
