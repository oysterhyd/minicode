package workspace

import (
	"math/rand/v2"
	"reflect"
	"strconv"
	"strings"
	"testing"

	"minicode.desktop/internal/model"
)

func referenceStatus132(output string) []model.Change {
	fields := strings.Split(output, "\x00")
	result := make([]model.Change, 0, len(fields))
	for i := 0; i < len(fields); i++ {
		record := fields[i]
		if len(record) < 4 {
			continue
		}
		status := record[:2]
		result = append(result, model.Change{Status: status, Path: record[3:]})
		if strings.ContainsAny(status, "RC") {
			i++
		}
	}
	return result
}

func referenceNumstat132(output string) map[string]Stat {
	fields := strings.Split(output, "\x00")
	result := make(map[string]Stat)
	for i := 0; i < len(fields); i++ {
		parts := strings.SplitN(fields[i], "\t", 3)
		if len(parts) != 3 {
			continue
		}
		path := parts[2]
		if path == "" {
			i += 2
			if i >= len(fields) {
				break
			}
			path = fields[i]
		}
		if path == "" {
			continue
		}
		a, _ := strconv.Atoi(parts[0])
		d, _ := strconv.Atoi(parts[1])
		old := result[path]
		result[path] = Stat{old.Additions + a, old.Deletions + d}
	}
	return result
}

func TestGitParsers132PreserveRenamesMalformedRecordsAndOrder(t *testing.T) {
	rng := rand.New(rand.NewPCG(132, 1))
	records := []string{"", "invalid", "3\t2\tfile", "-\t-\tbinary", "0\t1\t", "old", "new", "R  new", "?? file", " C copied", "?\n unusual\tname", "99999999999999999999999999\t2\tbig"}
	for range 1000 {
		var out strings.Builder
		for range rng.IntN(50) {
			out.WriteString(records[rng.IntN(len(records))])
			out.WriteByte(0)
		}
		text := out.String()
		if !reflect.DeepEqual(ParseStatus(text), referenceStatus132(text)) {
			t.Fatal("status order changed")
		}
		if !reflect.DeepEqual(ParseNumstat(text), referenceNumstat132(text)) {
			t.Fatalf("numstat changed for %q", text)
		}
	}
}

func BenchmarkGitStatus132(b *testing.B) {
	input := strings.Repeat("R  new name.txt\x00old name.txt\x00?? line\nname.txt\x00", 30000)
	b.ReportAllocs()
	for b.Loop() {
		if len(ParseStatus(input)) != 60000 {
			b.Fatal("wrong status count")
		}
	}
}

func BenchmarkGitNumstat132(b *testing.B) {
	input := strings.Repeat("3\t2\t\x00old name.txt\x00new name.txt\x00-\t-\tbinary.bin\x00", 30000)
	b.ReportAllocs()
	for b.Loop() {
		if ParseNumstat(input)["new name.txt"].Additions != 90000 {
			b.Fatal("wrong diff totals")
		}
	}
}
