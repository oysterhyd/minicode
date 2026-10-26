package main

import (
	"context"
	"encoding/json"
	"errors"
	"strings"
	"testing"

	"minicode.desktop/internal/model"
)

func TestResumeRefreshesMissingCachedSession(t *testing.T) {
	r := newTestRig(t, "home")
	base := r.d.client.(*fixtureClient)
	base.Sessions = []model.Session{{ID: "uncached-session", Workspace: r.d.current().Workspace}}
	r.d.sessions = nil
	reads := 0
	r.d.client = auditRequester(func(ctx context.Context, method string, params, target any) error {
		if method == "runSlash" {
			return json.Unmarshal([]byte(`{"action":"resume","sessionId":"uncached-session"}`), target)
		}
		if method == "listSessions" {
			reads++
		}
		return base.Request(ctx, method, params, target)
	})
	r.d.slash("/resume uncached-session", r.d.current())
	r.settle()
	if reads == 0 || r.d.current().SessionID != "uncached-session" || !r.d.current().Loaded {
		t.Fatal("uncached resume did not refresh and open the session")
	}
}

func TestResumeMissingOrFailedRefreshReportsError(t *testing.T) {
	for _, failure := range []bool{false, true} {
		t.Run(map[bool]string{false: "missing", true: "refresh-error"}[failure], func(t *testing.T) {
			r := newTestRig(t, "home")
			v := r.d.current()
			base := r.d.client.(*fixtureClient)
			r.d.sessions = nil
			r.d.client = auditRequester(func(ctx context.Context, method string, params, target any) error {
				if method == "runSlash" {
					return json.Unmarshal([]byte(`{"action":"resume","sessionId":"missing-session"}`), target)
				}
				if method == "listSessions" && failure {
					return errors.New("refresh failed")
				}
				return base.Request(ctx, method, params, target)
			})
			r.d.slash("/resume missing-session", v)
			r.settle()
			want := "missing-session"
			if failure {
				want = "refresh failed"
			}
			if r.d.current() != v || !strings.Contains(v.Error, want) {
				t.Fatalf("resume failed silently: %q", v.Error)
			}
		})
	}
}

func TestResumeRefreshCannotUndoANewerNavigation(t *testing.T) {
	r := newTestRig(t, "home")
	base := r.d.client.(*fixtureClient)
	r.d.sessions = nil
	entered, release := make(chan struct{}), make(chan struct{})
	r.d.client = auditRequester(func(ctx context.Context, method string, params, target any) error {
		if method == "runSlash" {
			return json.Unmarshal([]byte(`{"action":"resume","sessionId":"audit"}`), target)
		}
		if method == "listSessions" {
			close(entered)
			<-release
		}
		return base.Request(ctx, method, params, target)
	})
	r.d.slash("/resume audit", r.d.current())
	r.settle()
	select {
	case <-entered:
	default:
		close(release)
		t.Fatal("resume did not request a refreshed list")
	}
	r.d.newSession(r.d.current().Workspace)
	key := r.d.active
	close(release)
	r.settle()
	if r.d.active != key || r.d.current().SessionID != "" {
		t.Fatal("late resume stole newer navigation")
	}
}
