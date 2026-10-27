package main

import (
	"encoding/json"
	"minicode.desktop/internal/model"
	"testing"
)

func TestCustomModelDefaultsToConservativeContext(t *testing.T) {
	r := newTestRig(t, "home")
	r.d.openSettings("model")
	r.settle()
	click(t, r.ui, "添加服务")
	click(t, r.ui, "自定义模型 ID")
	r.ui.Type("unknown")
	click(t, r.ui, "添加")
	if got := r.d.settings.Service.Models[0].ContextWindow; got != 128000 {
		t.Fatalf("unknown model context window = %d", got)
	}
}

func TestServiceModelPreservesParameterCompatibility(t *testing.T) {
	var service model.Service
	input := []byte(`{"models":[{"modelId":"custom","parameterCompat":{"maxCompletionTokens":true,"reasoningEffort":{"max":"high"}}}]}`)
	if err := json.Unmarshal(input, &service); err != nil {
		t.Fatal(err)
	}
	output, err := json.Marshal(service)
	if err != nil {
		t.Fatal(err)
	}
	var restored map[string]any
	if err := json.Unmarshal(output, &restored); err != nil {
		t.Fatal(err)
	}
	entry := restored["models"].([]any)[0].(map[string]any)
	compat, ok := entry["parameterCompat"].(map[string]any)
	if !ok || compat["maxCompletionTokens"] != true {
		t.Fatalf("compatibility data was lost: %s", output)
	}
}
