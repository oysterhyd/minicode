package main

import (
	"fmt"
	"slices"
	"strings"
	"testing"

	"github.com/egoist/mygo/ui"
	"minicode.desktop/internal/model"
)

// Keep fixtures identical across revisions; run with -benchmem -count=5.
func BenchmarkOptimization(b *testing.B) {
	items := make([]*model.FeedItem, 30000, 31000)
	for i := range items {
		items[i] = &model.FeedItem{ID: fmtInt(i), Kind: "assistant", Text: "Completed response"}
	}
	rows := make([]model.FeedRow, len(items), 31000)
	copy(rows, model.GroupFeed(items))
	b.Run("Append30k", func(b *testing.B) {
		v := newConversation("bench", "", initialState())
		item := &model.FeedItem{ID: "next", Kind: "assistant"}
		b.ReportAllocs()
		b.ResetTimer()
		for range b.N {
			v.Items, v.Rows = items, rows
			v.append(item)
		}
	})
	b.Run("RemoveThinking30k", func(b *testing.B) {
		v := newConversation("bench", "", initialState())
		v.Items, v.Rows = items, rows
		b.ReportAllocs()
		b.ResetTimer()
		for range b.N {
			v.removeThinking()
		}
	})
	b.Run("MarkdownWarm24k", func(b *testing.B) {
		d := newDesktop(func(fn func()) { fn() })
		source := strings.Repeat("### Heading\n\nParagraph with **bold**, `code` and a [link](https://example.com).\n\n", 320)
		view := ui.NewTester(func(c *ui.Context) { d.markdown(c, d.theme(c), source) }, 820, 940)
		view.SetPreferences(ui.Preferences{ReduceMotion: true})
		view.Frame()
		b.ResetTimer()
		b.ReportAllocs()
		for range b.N {
			view.Frame()
		}
	})
	b.Run("MarkdownChunks24k", func(b *testing.B) {
		source := strings.Repeat("### Heading\n\nParagraph with **bold** and `code`.\n\n", 480)
		b.ReportAllocs()
		for range b.N {
			_ = markdownChunks(source)
		}
	})
	b.Run("HistoryUnmatched2k", func(b *testing.B) {
		var detail model.Detail
		for i := range 2000 {
			detail.Messages = append(detail.Messages, model.Message{Role: "assistant", Content: []map[string]any{{"type": "text", "text": fmt.Sprint("unmatched-", i)}}})
			detail.Events = append(detail.Events, model.Event{Type: "assistant_message", Seq: i, Data: map[string]any{"text": fmt.Sprint("response-", i), "reasoning": "reasoning"}})
		}
		b.ReportAllocs()
		b.ResetTimer()
		for range b.N {
			_ = model.History(detail)
		}
	})
	b.Run("HistoryMatched2k", func(b *testing.B) {
		var detail model.Detail
		for i := range 2000 {
			text := fmt.Sprint("response-", i)
			detail.Messages = append(detail.Messages, model.Message{Role: "assistant", Content: []map[string]any{{"type": "text", "text": text}}})
			detail.Events = append(detail.Events, model.Event{Type: "assistant_message", Seq: i, Data: map[string]any{"text": text, "reasoning": "reasoning"}})
		}
		b.ReportAllocs()
		b.ResetTimer()
		for range b.N {
			_ = model.History(detail)
		}
	})
}

func referenceSessionGroups(sessions []model.Session, current string, recent []string, query string) sessionGroups {
	groups := sessionGroups{Projects: map[string][]model.Session{}}
	query = strings.ToLower(strings.TrimSpace(query))
	matches := func(s model.Session) bool {
		return query == "" || strings.Contains(strings.ToLower(s.Title+" "+s.Workspace), query)
	}
	groups.Pinned = slices.DeleteFunc(slices.Clone(sessions), func(s model.Session) bool { return !s.Pinned || !matches(s) })
	if current != "" {
		groups.Roots = append(groups.Roots, current)
	}
	for _, session := range sessions {
		if !slices.Contains(groups.Roots, session.Workspace) {
			groups.Roots = append(groups.Roots, session.Workspace)
		}
	}
	for _, root := range recent {
		if !slices.Contains(groups.Roots, root) {
			groups.Roots = append(groups.Roots, root)
		}
	}
	for _, root := range groups.Roots {
		for _, session := range sessions {
			if session.Workspace == root && !session.Pinned && matches(session) {
				groups.Projects[root] = append(groups.Projects[root], session)
			}
		}
	}
	return groups
}

func BenchmarkSessionGroups(b *testing.B) {
	sessions := make([]model.Session, 10000)
	for i := range sessions {
		sessions[i] = model.Session{ID: fmtInt(i), Workspace: fmt.Sprint("project-", i%200), Pinned: i%31 == 0}
	}
	for _, run := range []struct {
		name string
		fn   func([]model.Session, string, []string, string) sessionGroups
	}{{"Before", referenceSessionGroups}, {"After", groupSessions}} {
		b.Run(run.name, func(b *testing.B) {
			b.ReportAllocs()
			for range b.N {
				_ = run.fn(sessions, "project-0", []string{"empty"}, "")
			}
		})
	}
}
