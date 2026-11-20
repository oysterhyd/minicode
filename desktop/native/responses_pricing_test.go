package main

import (
	"encoding/json"
	"strings"
	"testing"

	"minicode.desktop/internal/model"
)

func TestResponsesAndPricingJSONRoundTrip(t *testing.T) {
	service := model.Service{
		Name:     "OpenAI",
		BaseURL:  "https://api.openai.com/v1",
		APIStyle: "openai_responses",
		Enabled:  true,
		Models: []model.ServiceModel{{
			ModelID:       "gpt-5.4-pro",
			Name:          "GPT-5.4 Pro",
			ContextWindow: 1050000,
			MaxOutput:     128000,
			Pricing: &model.ModelPricing{
				Input:      2.5,
				Output:     10.0,
				CacheRead:  0.25,
				CacheWrite: 0.0,
			},
		}},
	}
	encoded, err := json.Marshal(service)
	if err != nil {
		t.Fatalf("marshal service: %v", err)
	}
	var decoded model.Service
	if err := json.Unmarshal(encoded, &decoded); err != nil {
		t.Fatalf("unmarshal service: %v", err)
	}
	if decoded.APIStyle != "openai_responses" {
		t.Fatalf("unexpected apiStyle: %q", decoded.APIStyle)
	}
	if decoded.Models[0].Pricing == nil || decoded.Models[0].Pricing.Input != 2.5 || decoded.Models[0].Pricing.Output != 10.0 {
		t.Fatalf("unexpected decoded pricing: %+v", decoded.Models[0].Pricing)
	}

	cost := 0.000084
	rawStats := `{"toolCalls":1,"requests":1,"modelSeconds":0.5,"cost":0.000084,"samples":[{"round":1,"input":100,"output":10,"cached":40,"available":true,"costUsd":0.000084}]}`
	var stats model.Statistics
	if err := json.Unmarshal([]byte(rawStats), &stats); err != nil {
		t.Fatalf("unmarshal statistics: %v", err)
	}
	if stats.Cost == nil || *stats.Cost != cost {
		t.Fatalf("unexpected stats cost: %+v", stats.Cost)
	}
	if len(stats.Samples) != 1 || stats.Samples[0].CostUSD == nil || *stats.Samples[0].CostUSD != cost {
		t.Fatalf("unexpected sample cost: %+v", stats.Samples)
	}

	if got := formatUSD(0); got != "$0.00" {
		t.Fatalf("formatUSD(0) = %q", got)
	}
	if got := formatUSD(0.000084); got != "$0.000084" {
		t.Fatalf("formatUSD(0.000084) = %q", got)
	}
	if got := formatUSD(0.123456); got != "$0.1235" {
		t.Fatalf("formatUSD(0.123456) = %q", got)
	}

	r := newTestRig(t, "conversation")
	r.d.openSettings("model")
	r.settle()
	click(t, r.ui, "编辑 CommandCode")
	s := r.d.settings
	s.Service.APIStyle = "openai_responses"
	s.Service.Models[0].Pricing = &model.ModelPricing{Input: 1.0, Output: 2.0, CacheRead: 0.1, CacheWrite: 0.0}
	s.OpenModels[0] = true
	r.ui.Frame()
	if !strings.Contains(s.Service.APIStyle, "openai_responses") {
		t.Fatalf("expected openai_responses style")
	}
}
