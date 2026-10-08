package main

import (
	"encoding/binary"
	"encoding/json"
	"path/filepath"
	"testing"
	"unicode/utf16"

	"github.com/syndtr/goleveldb/leveldb"
)

func TestLegacyPreferenceImportFromChromiumStorage(t *testing.T) {
	profile := t.TempDir()
	db, err := leveldb.OpenFile(filepath.Join(profile, "Local Storage", "leveldb"), nil)
	if err != nil {
		t.Fatal(err)
	}
	prefs := defaultPreferences()
	prefs.Theme = "dark"
	prefs.FontScale = 1.1
	prefs.SidebarWidth = 310
	data, _ := json.Marshal(prefs)
	key := []byte("_file://\x00\x01minicode.preferences.v2")
	if err := db.Put(key, append([]byte{1}, data...), nil); err != nil {
		t.Fatal(err)
	}
	history := []byte(`["原生迁移","修复测试"]`)
	units := utf16.Encode([]rune(string(history)))
	encoded := make([]byte, 1+len(units)*2)
	for i, u := range units {
		binary.LittleEndian.PutUint16(encoded[1+i*2:], u)
	}
	if err := db.Put([]byte("_file://\x00\x01minicode.prompt-history"), encoded, nil); err != nil {
		t.Fatal(err)
	}
	// Import from a copy also works while the original DB retains its lock.
	saved := savedPreferences{Preferences: defaultPreferences()}
	if !importLegacyPreferences(profile, &saved) {
		t.Fatal("legacy settings not imported")
	}
	if saved.Preferences.Theme != "dark" || saved.Preferences.FontScale != 1.1 || saved.Preferences.SidebarWidth != 310 {
		t.Fatal("preferences changed in migration")
	}
	if len(saved.History) != 2 || saved.History[0] != "原生迁移" {
		t.Fatal("UTF-16 history not imported")
	}
	_ = db.Close()
}
