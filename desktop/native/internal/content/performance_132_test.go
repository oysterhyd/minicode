package content

import (
	"strings"
	"testing"
)

func BenchmarkDiffParser132(b *testing.B) {
	input := "--- a/x\n+++ b/x\n@@ -1,10000 +1,10000 @@\n" + strings.Repeat(" context\n", 9999) + "-old\n+new\n"
	b.ReportAllocs()
	for b.Loop() {
		if len(ParseDiff(input)) != 10004 {
			b.Fatal("wrong diff length")
		}
	}
}
