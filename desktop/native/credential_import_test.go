package main

import (
	"image/png"
	"os"
	"path/filepath"
	"testing"
)

func TestCredentialImportDisclosesSourceAndRequiresAnExplicitConfirm(t *testing.T) {
	r := newTestRig(t, "conversation")
	r.d.openSettings("model")
	r.settle()
	before := len(r.d.config.Services)
	click(t, r.ui, "从 Command Code 导入")
	r.settle()
	for _, text := range []string{"provider_config.json", "API Key", "系统安全存储"} {
		if !r.ui.HasText(text) {
			t.Fatalf("missing import disclosure: %s", text)
		}
	}
	preview := filepath.Join("..", "..", "output", "native", "credential-import-dark-1x.png")
	if err := os.MkdirAll(filepath.Dir(preview), 0755); err != nil {
		t.Fatal(err)
	}
	file, err := os.Create(preview)
	if err != nil {
		t.Fatal(err)
	}
	if err := png.Encode(file, r.ui.Image()); err != nil {
		file.Close()
		t.Fatal(err)
	}
	if err := file.Close(); err != nil {
		t.Fatal(err)
	}
	if len(r.d.config.Services) != before {
		t.Fatal("import ran before confirmation")
	}
	click(t, r.ui, "取消导入")
	r.settle()
	if len(r.d.config.Services) != before {
		t.Fatal("cancel imported credentials")
	}
	click(t, r.ui, "从 Command Code 导入")
	r.settle()
	click(t, r.ui, "确认读取并导入")
	r.settle()
	if len(r.d.config.Services) != before+1 {
		t.Fatal("confirmed import did not add a service")
	}
}
