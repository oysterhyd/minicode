package main

import "testing"

func TestPowerShellUsesCommandPresentation(t *testing.T) {
	meta, ok := toolNames["powershell"]
	if !ok || meta.Icon != "Terminal" {
		t.Fatalf("PowerShell must use the terminal presentation: %+v", meta)
	}
}
