package main

import (
	"encoding/json"
	"fmt"
	"os"
	"sort"
	"time"

	"github.com/egoist/mygo"
	"minicode.desktop/internal/bridge"
)

// This measures controller dispatch through native view construction. Layout,
// presentation and animation duration are deliberately outside this metric.
func runUIBenchmark(d *desktop, path string) {
	mygo.RunOnMain(func() { d.window.ShowInactive() })
	results := map[string]any{}
	frame := func(change func(), ready ...func() bool) (float64, error) {
		done := make(chan float64, 1)
		start := time.Now()
		d.window.Update(func() {
			change()
			d.frameObserved = func() {
				if len(ready) > 0 && !ready[0]() {
					return
				}
				d.frameObserved = nil
				done <- float64(time.Since(start).Nanoseconds()) / 1e6
			}
		})
		select {
		case elapsed := <-done:
			return elapsed, nil
		case <-time.After(5 * time.Second):
			return 0, fmt.Errorf("native view timed out")
		}
	}
	// Wait for font loading and the initial entrance motions before sampling.
	time.Sleep(700 * time.Millisecond)
benchmarks:
	for _, name := range []string{"work_tab", "settings_open", "context_open", "stream_delta"} {
		values := make([]float64, 7)
		for i := -2; i < 7; i++ {
			_, err := frame(func() {
				d.views[d.active] = newConversation(d.active, "", initialState())
				d.tab = 0
				loadFixture(d, "home")
				d.settings, d.preview = nil, nil
				d.contextOpen = false
				d.current().Items = nil
				d.current().rebuildRows()
				if name == "stream_delta" {
					loadFixture(d, "conversation")
				}
			})
			if err != nil {
				results["error"] = err.Error()
				break benchmarks
			}
			elapsed, err := frame(func() {
				switch name {
				case "work_tab":
					d.tab = 1
				case "settings_open":
					d.openSettings("general")
				case "context_open":
					d.contextOpen = true
				case "stream_delta":
					d.queueEvent(bridge.Event{Event: "text_delta", ClientKey: d.active, Text: "New streamed text"})
				}
			}, func() bool { return name != "stream_delta" || d.current().stream != nil })
			if err != nil {
				results["error"] = err.Error()
				break benchmarks
			}
			if i >= 0 {
				values[i] = elapsed
			}
		}
		sort.Float64s(values)
		results[name] = map[string]any{"median_ms": values[3], "min_ms": values[0], "max_ms": values[6], "repeats": 7}
	}
	data, _ := json.MarshalIndent(map[string]any{"metric": "controller dispatch to view construction; excludes layout, paint and animation", "benchmarks": results}, "", "  ")
	_ = os.WriteFile(path, append(data, '\n'), 0600)
	mygo.App.Quit()
}
