package main

import (
	"minicode.desktop/internal/bridge"
	"testing"
)

func TestProtocolWarningIsNonfatal(t *testing.T) {
	r := newTestRig(t, "home")
	v := r.d.current()
	v.Busy = true
	r.d.handleEvent(bridge.Event{Event: "warning", Text: "optional extension unavailable"})
	if !v.Busy || v.Error != "" || len(v.Items) == 0 || v.Items[len(v.Items)-1].Tone != "warning" {
		t.Fatal("warning used the error channel")
	}
}
