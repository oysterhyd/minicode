package main

import (
	"encoding/json"
	"fmt"
	"os"
	"runtime"
	"sort"
	"strings"
	"testing"

	"minicode.desktop/internal/content"
	"minicode.desktop/internal/model"
)

type measurement struct {
	Median     float64 `json:"median_ms"`
	Min        float64 `json:"min_ms"`
	Max        float64 `json:"max_ms"`
	Repeats    int     `json:"repeats"`
	OutputRows int     `json:"output_rows,omitempty"`
	Additions  int     `json:"additions,omitempty"`
	Deletions  int     `json:"deletions,omitempty"`
}

func measured(fn func()) measurement {
	fn()
	fn()
	values := make([]float64, 7)
	for i := range values {
		runtime.GC()
		start := preciseMilliseconds()
		fn()
		values[i] = preciseMilliseconds() - start
	}
	sort.Float64s(values)
	return measurement{Median: values[3], Min: values[0], Max: values[6], Repeats: 7}
}

func TestComparableAlgorithmMeasurements(t *testing.T) {
	output := os.Getenv("MINICODE_BENCH_OUTPUT")
	if output == "" {
		t.Skip("set MINICODE_BENCH_OUTPUT to save benchmark evidence")
	}
	items := make([]*model.FeedItem, 30000)
	for i := range items {
		kind := "user"
		if i%2 == 1 {
			kind = "assistant"
		}
		items[i] = &model.FeedItem{ID: fmtInt(i), Kind: kind}
	}
	makeTexts := func(n int, edits []int) (string, string) {
		a := make([]string, n)
		for i := range a {
			a[i] = fmt.Sprintf("line-%d", i)
		}
		b := append([]string(nil), a...)
		for _, i := range edits {
			b[i] = fmt.Sprintf("edit-%d", i)
		}
		return strings.Join(a, "\n"), strings.Join(b, "\n")
	}
	a, b := makeTexts(400, []int{100, 300})
	largeA, largeB := makeTexts(10000, []int{100, 5000, 9900})
	results := map[string]measurement{}
	results["feed_group_and_positions_30k_rows"] = measured(func() {
		sum := 0
		for _, row := range model.GroupFeed(items) {
			if row.Item != nil {
				sum += row.Position
			}
		}
		if sum != 30000*29999/2 {
			t.Fatal("source positions changed")
		}
	})
	results["diff_400_lines_two_edits"] = measured(func() { _ = content.DiffTexts(a, b) })
	large := measured(func() { _ = content.DiffTexts(largeA, largeB) })
	lines := content.DiffTexts(largeA, largeB)
	large.OutputRows = len(lines)
	large.Additions, large.Deletions = content.DiffStats(lines)
	results["diff_10k_lines_three_edits"] = large
	data, err := json.MarshalIndent(map[string]any{"go": runtime.Version(), "platform": runtime.GOOS, "benchmarks": results}, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(output, append(data, '\n'), 0600); err != nil {
		t.Fatal(err)
	}
}

func BenchmarkNativeView30kMessages(b *testing.B) {
	d := newDesktop(func(fn func()) { fn() })
	loadFixture(d, "conversation")
	v := d.current()
	v.Items = make([]*model.FeedItem, 30000)
	for i := range v.Items {
		v.Items[i] = &model.FeedItem{ID: fmtInt(i), Kind: "assistant", Text: "A short completed message."}
	}
	v.rebuildRows()
	b.ResetTimer()
	for range b.N {
		_ = model.GroupFeed(v.Items)
	}
}
