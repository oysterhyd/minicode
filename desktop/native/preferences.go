package main

import (
	"encoding/json"
	"os"
	"path/filepath"
	"runtime"
	"slices"
	"strings"
	"sync"
	"time"
)

type preferences struct {
	Theme          string  `json:"theme"`
	SendKey        string  `json:"sendKey"`
	Notifications  bool    `json:"notifications"`
	FontScale      float32 `json:"fontScale"`
	LeftCollapsed  bool    `json:"leftCollapsed"`
	RightCollapsed bool    `json:"rightCollapsed"`
	SidebarWidth   float32 `json:"sidebarWidth"`
	WorkWidth      float32 `json:"workWidth"`
	ExpandTools    bool    `json:"expandTools"`
}

func defaultPreferences() preferences {
	return preferences{Theme: "system", SendKey: "enter", Notifications: true, FontScale: 1, SidebarWidth: 268, WorkWidth: 400}
}

type savedPreferences struct {
	Preferences preferences `json:"preferences"`
	Workspace   string      `json:"workspace"`
	Recent      []string    `json:"recent"`
	History     []string    `json:"history"`
}

type preferenceStore struct {
	path    string
	mu      sync.Mutex
	timer   *time.Timer
	pending []byte
}

func (d *desktop) loadPreferences(dir string) {
	d.store = &preferenceStore{path: filepath.Join(dir, "native-preferences.json")}
	saved := savedPreferences{Preferences: defaultPreferences()}
	data, err := os.ReadFile(d.store.path)
	if err == nil {
		_ = json.Unmarshal(data, &saved)
	} else {
		// The old host's workspace file is plain JSON and survives migration.
		config, _ := os.UserConfigDir()
		for _, name := range []string{"MiniCode", "minicode", "minicode-desktop"} {
			profile := filepath.Join(config, name)
			old, err := os.ReadFile(filepath.Join(profile, "workspace.json"))
			if err != nil {
				continue
			}
			_ = json.Unmarshal(old, &saved)
			_ = importLegacyPreferences(profile, &saved)
			if saved.Workspace != "" {
				break
			}
		}
	}
	if saved.Preferences.FontScale < 0.8 || saved.Preferences.FontScale > 1.4 {
		saved.Preferences.FontScale = 1
	}
	saved.Preferences.SidebarWidth = max(200, min(400, saved.Preferences.SidebarWidth))
	saved.Preferences.WorkWidth = max(280, min(680, saved.Preferences.WorkWidth))
	d.preferences = saved.Preferences
	d.root = saved.Workspace
	d.recent = saved.Recent
	d.history = saved.History
	d.current().Workspace = d.root
	if err != nil {
		d.persist()
	}
}

func (d *desktop) persist() {
	if d.store == nil {
		return
	}
	data, err := json.Marshal(savedPreferences{d.preferences, d.root, d.recent, d.history})
	if err != nil {
		return
	}
	s := d.store
	s.mu.Lock()
	s.pending = data
	if s.timer != nil {
		s.timer.Stop()
	}
	s.timer = time.AfterFunc(120*time.Millisecond, s.flush)
	s.mu.Unlock()
}

func (s *preferenceStore) flush() {
	s.mu.Lock()
	defer s.mu.Unlock()
	if len(s.pending) == 0 {
		return
	}
	if os.MkdirAll(filepath.Dir(s.path), 0700) != nil {
		return
	}
	temp := s.path + ".tmp"
	if os.WriteFile(temp, s.pending, 0600) == nil {
		_ = os.Rename(temp, s.path)
	}
	s.pending = nil
}

func samePath(a, b string) bool {
	if runtime.GOOS == "windows" {
		return strings.EqualFold(a, b)
	}
	return a == b
}

func (d *desktop) rememberWorkspace(root string) {
	if root == "" {
		return
	}
	d.root = root
	d.recent = slices.DeleteFunc(d.recent, func(item string) bool { return samePath(item, root) })
	d.recent = append([]string{root}, d.recent...)
	if len(d.recent) > 12 {
		d.recent = d.recent[:12]
	}
	d.persist()
}

func (d *desktop) rememberPrompt(text string) {
	d.history = slices.DeleteFunc(d.history, func(item string) bool { return item == text })
	d.history = append([]string{text}, d.history...)
	if len(d.history) > 50 {
		d.history = d.history[:50]
	}
	d.persist()
}
