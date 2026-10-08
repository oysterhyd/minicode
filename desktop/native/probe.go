package main

import (
	"context"
	"encoding/json"
	"os"
	"time"

	"github.com/egoist/mygo"
	"minicode.desktop/internal/model"
)

func runInstalledProbe(d *desktop, path string, started time.Time) {
	report := map[string]any{"version": appVersion, "native": d.window.Page() == nil}
	report["updates_enabled"] = mygo.Updater.Enabled()
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	ready := make(chan float64, 1)
	var poll func()
	poll = func() {
		if d.current().Loaded || d.current().Error != "" {
			ready <- float64(time.Since(started).Nanoseconds()) / 1e6
			return
		}
		time.AfterFunc(5*time.Millisecond, func() { d.schedule(poll) })
	}
	d.schedule(poll)
	select {
	case elapsed := <-ready:
		report["initialize_ms"] = elapsed
	case <-ctx.Done():
		report["error"] = "initialization timeout"
	}
	var state model.State
	if d.client == nil {
		report["error"] = "Python bridge unavailable"
	} else if err := d.client.Request(ctx, "getState", map[string]any{"clientKey": "installed-probe"}, &state); err != nil {
		report["error"] = err.Error()
	} else {
		report["protocol"] = state.ProtocolVersion
		report["phase"] = state.Phase
	}
	pixels, err := d.window.CapturePage()
	if err != nil {
		report["capture_error"] = err.Error()
	} else {
		report["capture_bytes"] = len(pixels)
		_ = os.WriteFile(path+".png", pixels, 0600)
	}
	data, _ := json.MarshalIndent(report, "", "  ")
	_ = os.WriteFile(path, data, 0600)
	mygo.App.Quit()
}
