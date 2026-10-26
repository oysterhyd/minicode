package main

import (
	"testing"

	"minicode.desktop/internal/model"
)

func TestResumeCompletionKeepsFullIDsWithTheSamePrefix(t *testing.T) {
	r := newTestRig(t, "home")
	r.d.sessions = []model.Session{{ID: "12345678-first-session", Title: "first"}, {ID: "12345678-second-session", Title: "second"}}
	v := r.d.current()
	v.Draft = "/resume 12345678"
	for index, session := range r.d.sessions {
		r.d.completion = r.d.suggestions(v, len([]rune(v.Draft)))
		if r.d.completion == nil || len(r.d.completion.Items) != 2 {
			t.Fatal("prefix did not find both sessions")
		}
		text, _, send := r.d.pickSuggestion(v.Draft, index)
		if text != "/resume "+session.ID || !send {
			t.Fatalf("ambiguous resume completion: %q", text)
		}
	}
}
