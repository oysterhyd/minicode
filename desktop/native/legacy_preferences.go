package main

import (
	"encoding/binary"
	"encoding/json"
	"io"
	"os"
	"path/filepath"
	"strings"
	"unicode/utf16"

	"github.com/syndtr/goleveldb/leveldb"
	"github.com/syndtr/goleveldb/leveldb/opt"
)

func chromiumString(value []byte) string {
	if len(value) == 0 {
		return ""
	}
	switch value[0] {
	case 1:
		runes := make([]rune, len(value)-1)
		for i, b := range value[1:] {
			runes[i] = rune(b)
		}
		return string(runes)
	case 0:
		value = value[1:]
		units := make([]uint16, len(value)/2)
		for i := range units {
			units[i] = binary.LittleEndian.Uint16(value[i*2:])
		}
		return string(utf16.Decode(units))
	default:
		return string(value)
	}
}

func importLegacyPreferences(profile string, saved *savedPreferences) bool {
	source := filepath.Join(profile, "Local Storage", "leveldb")
	entries, err := os.ReadDir(source)
	if err != nil {
		return false
	}
	temp, err := os.MkdirTemp("", "minicode-preferences-migration-")
	if err != nil {
		return false
	}
	full, _ := filepath.Abs(temp)
	base, _ := filepath.Abs(os.TempDir())
	if !samePath(filepath.Dir(full), base) || !strings.HasPrefix(filepath.Base(full), "minicode-preferences-migration-") {
		return false
	}
	defer os.RemoveAll(full)
	total := int64(0)
	for _, entry := range entries {
		if !entry.Type().IsRegular() {
			continue
		}
		name := entry.Name()
		if name == "LOCK" || name == "LOG" || name == "LOG.old" {
			continue
		}
		info, err := entry.Info()
		if err != nil {
			return false
		}
		total += info.Size()
		if total > 32<<20 {
			return false
		}
		input, err := os.Open(filepath.Join(source, name))
		if err != nil {
			return false
		}
		output, err := os.OpenFile(filepath.Join(full, name), os.O_CREATE|os.O_WRONLY|os.O_EXCL, 0600)
		if err != nil {
			_ = input.Close()
			return false
		}
		_, err = io.Copy(output, input)
		_ = input.Close()
		_ = output.Close()
		if err != nil {
			return false
		}
	}
	db, err := leveldb.OpenFile(full, &opt.Options{ReadOnly: true, ErrorIfMissing: true})
	if err != nil {
		return false
	}
	defer db.Close()
	found := false
	iter := db.NewIterator(nil, nil)
	defer iter.Release()
	for iter.Next() {
		key := string(iter.Key())
		switch {
		case strings.HasSuffix(key, "minicode.preferences.v2"):
			prefs := defaultPreferences()
			if json.Unmarshal([]byte(chromiumString(iter.Value())), &prefs) == nil {
				saved.Preferences = prefs
				found = true
			}
		case strings.HasSuffix(key, "minicode.prompt-history"):
			var history []string
			if json.Unmarshal([]byte(chromiumString(iter.Value())), &history) == nil {
				saved.History = history[:min(50, len(history))]
			}
		}
	}
	return found
}
