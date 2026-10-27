package model

import (
	"encoding/json"
	"strings"
	"testing"
)

func TestMCPDiscoveryPreservesOptionalDiagnostics(t *testing.T) {
	var discovery Discovery
	data := `{"servers":[{"server":"test","error":"failed","stderr_tail":"last diagnostic","log_path":"C:\\logs\\mcp.log"}],"tools":[]}`
	if err := json.Unmarshal([]byte(data), &discovery); err != nil {
		t.Fatal(err)
	}
	encoded, err := json.Marshal(discovery)
	if err != nil {
		t.Fatal(err)
	}
	for _, expected := range []string{"last diagnostic", "mcp.log"} {
		if !strings.Contains(string(encoded), expected) {
			t.Fatalf("diagnostic %q was lost: %s", expected, encoded)
		}
	}
	if err := json.Unmarshal([]byte(`{"servers":[{"server":"old","error":null}],"tools":[]}`), &discovery); err != nil {
		t.Fatalf("legacy discovery must remain readable: %v", err)
	}
}
