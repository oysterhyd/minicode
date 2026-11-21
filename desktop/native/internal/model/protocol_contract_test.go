package model

import (
	"encoding/json"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
)

type protocolManifest struct {
	ProtocolVersion   int      `json:"protocolVersion"`
	MinClientProtocol int      `json:"minClientProtocol"`
	StateFields       []string `json:"stateFields"`
	Conformance       struct {
		RunGateCases []struct {
			Name string `json:"name"`
			View struct {
				SessionID   string   `json:"sessionId"`
				Abandoned   []string `json:"abandoned"`
				RunID       string   `json:"runId"`
				AwaitingRun bool     `json:"awaitingRun"`
				Busy        bool     `json:"busy"`
			} `json:"view"`
			Event struct {
				Event     string `json:"event"`
				SessionID string `json:"sessionId"`
				RunID     string `json:"runId"`
			} `json:"event"`
			Expected bool `json:"expected"`
		} `json:"runGateCases"`
		NoticeCases []struct {
			Name         string         `json:"name"`
			Type         string         `json:"type"`
			Data         map[string]any `json:"data"`
			ExpectedText string         `json:"expectedText"`
			ExpectedTone string         `json:"expectedTone"`
		} `json:"noticeCases"`
		ToolOutputCases []struct {
			Name     string         `json:"name"`
			Data     map[string]any `json:"data"`
			Expected string         `json:"expected"`
		} `json:"toolOutputCases"`
	} `json:"conformance"`
}

func loadProtocolManifest(t *testing.T) protocolManifest {
	t.Helper()
	path := filepath.Join("..", "..", "..", "..", "agent", "assets", "bridge-protocol.v2.json")
	raw, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("read protocol manifest %s: %v", path, err)
	}
	var manifest protocolManifest
	if err := json.Unmarshal(raw, &manifest); err != nil {
		t.Fatalf("decode protocol manifest: %v", err)
	}
	return manifest
}

func TestBridgeProtocolContract(t *testing.T) {
	manifest := loadProtocolManifest(t)
	if manifest.ProtocolVersion != 2 {
		t.Fatalf("protocolVersion = %d, want 2", manifest.ProtocolVersion)
	}
	if manifest.MinClientProtocol != 2 {
		t.Fatalf("minClientProtocol = %d, want 2", manifest.MinClientProtocol)
	}

	jsonFields := make(map[string]bool)
	stateType := reflect.TypeOf(State{})
	for i := range stateType.NumField() {
		tag := stateType.Field(i).Tag.Get("json")
		name := strings.Split(tag, ",")[0]
		if name != "" && name != "-" {
			jsonFields[name] = true
		}
	}
	for _, field := range manifest.StateFields {
		if !jsonFields[field] {
			t.Fatalf("model.State is missing json field %q required by protocol manifest", field)
		}
	}

	for _, tc := range manifest.Conformance.RunGateCases {
		got := AcceptsRunEvent(
			tc.View.SessionID,
			tc.View.Abandoned,
			tc.View.RunID,
			tc.View.AwaitingRun,
			tc.View.Busy,
			tc.Event.SessionID,
			tc.Event.RunID,
			tc.Event.Event,
		)
		if got != tc.Expected {
			t.Errorf("runGateCase %q: got %v, want %v", tc.Name, got, tc.Expected)
		}
	}

	for _, tc := range manifest.Conformance.NoticeCases {
		text, tone := NoticeForAgentEvent(tc.Type, tc.Data)
		if text != tc.ExpectedText || tone != tc.ExpectedTone {
			t.Errorf("noticeCase %q: got (%q, %q), want (%q, %q)", tc.Name, text, tone, tc.ExpectedText, tc.ExpectedTone)
		}
	}

	for _, tc := range manifest.Conformance.ToolOutputCases {
		got := FormatToolOutput(tc.Data)
		if got != tc.Expected {
			t.Errorf("toolOutputCase %q: got %q, want %q", tc.Name, got, tc.Expected)
		}
	}
}
