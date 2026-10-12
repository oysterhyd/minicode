package main

import (
	"minicode.desktop/internal/model"
	"reflect"
	"strings"
	"testing"
)

func TestIncrementalFeedMatchesHistoryGrouping(t *testing.T) {
	v := newConversation("test", "", initialState())
	for i := range 300 {
		kind := []string{"user", "tool", "tool", "assistant", "reasoning", "thinking"}[i%6]
		item := &model.FeedItem{ID: fmtInt(i), Kind: kind}
		v.append(item)
		if !reflect.DeepEqual(v.Rows, model.GroupFeed(v.Items)) {
			t.Fatalf("grouping changed at %d", i)
		}
		if kind == "tool" && v.toolItems[item.ID] != item {
			t.Fatal("tool index missing")
		}
	}
	v.removeThinking()
	if !reflect.DeepEqual(v.Rows, model.GroupFeed(v.Items)) || v.thinkingCount != 0 {
		t.Fatal("thinking removal changed groups")
	}
	v.Items = nil
	v.rebuildRows()
	if len(v.toolItems) != 0 {
		t.Fatal("clear retained stale tool references")
	}
}

// The previous splitter is the behavior oracle, including fences and blank
// lines after list markers. The optimized version returns slices of source.
func referenceMarkdownChunks(source string) []string {
	var chunks []string
	lines := strings.Split(source, "\n")
	start, fence := 0, ""
	for i, line := range lines {
		trim := strings.TrimSpace(line)
		if strings.HasPrefix(trim, "```") || strings.HasPrefix(trim, "~~~") {
			marker := trim[:3]
			if fence == "" {
				fence = marker
			} else if strings.HasPrefix(trim, fence) && strings.Trim(trim, "`~ ") == "" {
				fence = ""
			}
		}
		if fence == "" && trim == "" && i > start {
			previous := strings.TrimSpace(lines[i-1])
			if !strings.HasPrefix(previous, "- ") && !strings.HasPrefix(previous, "* ") && !strings.HasPrefix(previous, "+ ") {
				chunks = append(chunks, strings.Join(lines[start:i], "\n"))
				start = i + 1
			}
		}
	}
	if start < len(lines) {
		chunks = append(chunks, strings.Join(lines[start:], "\n"))
	}
	if len(chunks) == 0 {
		return []string{source}
	}
	return chunks
}

func TestMarkdownSplitterAndCachePreserveDocuments(t *testing.T) {
	lines := []string{"", " ", "\r", "段落 **文字**", "- item", "* item", "+ item", "```go", "```", "~~~", "### Title", "[link]: https://example.com"}
	cache := newRenderCache()
	for i := range 1000 {
		parts := make([]string, 20)
		for j := range parts {
			parts[j] = lines[(i*7+j*j+j)%len(lines)]
		}
		source := strings.Join(parts, "\n")
		want := referenceMarkdownChunks(source)
		if !reflect.DeepEqual(markdownChunks(source), want) {
			t.Fatalf("chunks changed for %q", source)
		}
		var expected document
		for _, chunk := range want {
			expected.Blocks = append(expected.Blocks, parseMarkdown(chunk).Blocks...)
		}
		got := cache.markdown(source)
		if !reflect.DeepEqual(got.Blocks, expected.Blocks) {
			t.Fatalf("document changed for %q", source)
		}
		if !reflect.DeepEqual(cache.markdown(source), got) {
			t.Fatal("cache changed document")
		}
	}
	for i := range 600 {
		cache.markdown("first\n\n" + fmtInt(i) + "\n\nlast")
	}
	if len(cache.docs) > 512 || cache.bytes > 8<<20 {
		t.Fatal("markdown cache grew without bound")
	}
}

func TestSessionGroupingPreservesRootsPinsAndSearch(t *testing.T) {
	sessions := []model.Session{{ID: "1", Workspace: "B", Pinned: true}, {ID: "2", Workspace: "A", Title: "MATCH"}, {ID: "3", Workspace: "B", Title: "other"}, {ID: "4", Workspace: "A", Title: "match"}}
	groups := groupSessions(sessions, "A", []string{"C", "B"}, "")
	if !reflect.DeepEqual(groups.Roots, []string{"A", "B", "C"}) || len(groups.Pinned) != 1 || groups.Projects["A"][1].ID != "4" {
		t.Fatal("session order changed")
	}
	groups = groupSessions(sessions, "A", []string{"C"}, " MaTcH ")
	if len(groups.Pinned) != 0 || len(groups.Projects["A"]) != 2 || len(groups.Projects["B"]) != 0 || len(groups.Roots) != 3 {
		t.Fatal("search changed roots or matches")
	}
}

func TestTraceRetainsOnlyDisplayedHistory(t *testing.T) {
	events := make([]model.Event, 10000)
	for i := range events {
		events[i] = model.Event{Seq: i, Data: map[string]any{"payload": "data"}}
	}
	trace := recentTrace(events)
	if len(trace) != 300 || cap(trace) >= 600 || trace[0].Seq != 9700 || trace[299].Seq != 9999 {
		t.Fatal("trace order or retention changed")
	}
	trace[0] = model.Event{}
	if events[9700].Data == nil {
		t.Fatal("trace still shares the source backing array")
	}
	d := newDesktop(func(fn func()) { fn() })
	v := d.current()
	v.Trace = recentTrace(events)
	old := v.Trace
	d.runtimeEvent(v, model.Event{Seq: 10000, Type: "diagnostic", Data: map[string]any{}})
	if len(v.Trace) != 300 || v.Trace[0].Seq != 9701 {
		t.Fatal("live trace order changed")
	}
	// append may grow first; exercise a retained backing slot without growth.
	v.Trace = make([]model.Event, 300, 400)
	v.Trace[0] = model.Event{Data: map[string]any{"large": "payload"}}
	old = v.Trace
	d.runtimeEvent(v, model.Event{Seq: 10001, Type: "diagnostic"})
	if old[0].Data != nil {
		t.Fatal("live trace kept a discarded payload")
	}
}
