package main

import (
	"minicode.desktop/internal/model"
	"strings"
)

type sessionGroups struct {
	Roots    []string
	Pinned   []model.Session
	Projects map[string][]model.Session
}

// Preserve first-seen workspace and session order in one pass, including empty
// recent workspaces and roots whose only sessions are pinned or filtered out.
func groupSessions(sessions []model.Session, current string, recent []string, query string) sessionGroups {
	groups := sessionGroups{Projects: map[string][]model.Session{}}
	seen := map[string]bool{}
	addRoot := func(root string) {
		if !seen[root] {
			seen[root] = true
			groups.Roots = append(groups.Roots, root)
		}
	}
	if current != "" {
		addRoot(current)
	}
	query = strings.ToLower(strings.TrimSpace(query))
	for _, session := range sessions {
		addRoot(session.Workspace)
		if query != "" && !strings.Contains(strings.ToLower(session.Title+" "+session.Workspace), query) {
			continue
		}
		if session.Pinned {
			groups.Pinned = append(groups.Pinned, session)
		} else {
			groups.Projects[session.Workspace] = append(groups.Projects[session.Workspace], session)
		}
	}
	for _, root := range recent {
		addRoot(root)
	}
	return groups
}
