package content

import (
	"fmt"
	"strings"
	"testing"
)

func TestDiffRetainsSeparatedEdits(t *testing.T) {
	a := make([]string, 10000)
	for i := range a {
		a[i] = fmt.Sprintf("line-%d", i)
	}
	b := append([]string(nil), a...)
	for _, i := range []int{100, 5000, 9900} {
		b[i] = "changed"
	}
	lines := DiffTexts(strings.Join(a, "\n"), strings.Join(b, "\n"))
	add, remove := DiffStats(lines)
	if add != 3 || remove != 3 || len(lines) != 10003 {
		t.Fatalf("%d additions, %d deletions, %d rows", add, remove, len(lines))
	}
}

func TestUnifiedDiffFileHeadersAndRenumbering(t *testing.T) {
	lines := ParseDiff("--- a/a\n+++ b/a\n@@ -3,2 +3,3 @@\n same\n-old\n+new\n+added\n")
	add, remove := DiffStats(lines)
	if add != 2 || remove != 1 {
		t.Fatalf("headers counted as content: %d/%d", add, remove)
	}
	if lines[len(lines)-1].NewLine != 5 {
		t.Fatal("new line number wrong")
	}
}

func TestDiffWorkCap(t *testing.T) {
	a := strings.Repeat("a\n", 10000)
	b := strings.Repeat("b\n", 10000)
	lines := DiffTexts(a, b)
	add, remove := DiffStats(lines)
	if add != 10000 || remove != 10000 {
		t.Fatalf("%d/%d", add, remove)
	}
}
