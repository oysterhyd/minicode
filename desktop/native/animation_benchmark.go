package main

import (
	"encoding/json"
	"os"
	"runtime"
	"sort"
	"strings"
	"time"

	"github.com/egoist/mygo"
	"minicode.desktop/internal/bridge"
	"minicode.desktop/internal/model"
)

// Construction timestamps span the previous frame's layout, paint, presentation
// and scheduler wait. This measures native frame cadence, not monitor scanout.
func runAnimationBenchmark(d *desktop, path string) {
	results := map[string]any{}
	for _, name := range []string{"running", "streaming_long", "panels"} {
		var intervals, builds []float64
		var previous float64
		start := time.Now().Add(600 * time.Millisecond)
		end := start.Add(3 * time.Second)
		mygo.RunOnMain(func() {
			d.settings, d.preview = nil, nil
			d.preferences.LeftCollapsed = false
			loadFixture(d, "conversation")
			v := d.current()
			v.Busy = true
			v.StartedAt = time.Now()
			if name == "streaming_long" {
				v.Items = make([]*model.FeedItem, 30000)
				for i := range v.Items {
					v.Items[i] = &model.FeedItem{ID: fmtInt(i), Kind: "assistant", Text: "Completed response"}
				}
				v.rebuildRows()
				d.handleEvent(bridge.Event{Event: "text_delta", ClientKey: d.active, Text: strings.Repeat("A paragraph with **bold** and `code`.\n\n", 300)})
				v.List.ScrollToEnd()
			}
			d.frameObserved = func() {
				if now := time.Now(); !now.Before(start) && now.Before(end) {
					builds = append(builds, preciseMilliseconds()-d.frameStarted)
				}
			}
			d.framePainted = func() {
				now := time.Now()
				stamp := preciseMilliseconds()
				if !now.Before(start) && now.Before(end) && previous != 0 {
					intervals = append(intervals, stamp-previous)
				}
				previous = stamp
			}
			d.window.ShowInactive()
			d.window.Invalidate()
		})
		tick := time.NewTicker(20 * time.Millisecond)
		nextPanel := time.Now()
		for time.Now().Before(end) {
			<-tick.C
			if name == "streaming_long" {
				d.queueEvent(bridge.Event{Event: "text_delta", ClientKey: d.active, Text: " streamed"})
			} else if name == "panels" && time.Now().After(nextPanel) {
				d.schedule(func() { d.preferences.LeftCollapsed = !d.preferences.LeftCollapsed })
				nextPanel = time.Now().Add(400 * time.Millisecond)
			}
		}
		tick.Stop()
		mygo.RunOnMain(func() {
			d.frameObserved = nil
			d.framePainted = nil
			missed := 0
			for _, interval := range intervals {
				if interval > 25 {
					missed++
				}
			}
			results[name] = map[string]any{"interval_ms": distribution(intervals), "build_ms": distribution(builds), "frames": len(intervals), "view_builds": len(builds), "intervals_over_25ms": missed}
		})
	}
	data, _ := json.MarshalIndent(map[string]any{"metric": "native frame cadence; not display FPS", "go": runtime.Version(), "platform": runtime.GOOS, "benchmarks": results}, "", "  ")
	_ = os.WriteFile(path, append(data, '\n'), 0600)
	mygo.App.Quit()
}

func distribution(values []float64) map[string]float64 {
	if len(values) == 0 {
		return nil
	}
	sort.Float64s(values)
	var total float64
	for _, value := range values {
		total += value
	}
	return map[string]float64{"median": values[len(values)/2], "p95": values[(len(values)-1)*95/100], "max": values[len(values)-1], "mean": total / float64(len(values))}
}
