package main

import (
	"reflect"
	"strings"
	"testing"
)

func TestMarkdownParserReuseKeepsDocumentsIndependent(t *testing.T) {
	source := "## 中文\n\n[reference][r]\n\n[r]: https://example.test\n\n```go\n// 中文\nfunc main() {}\n```"
	original := parseMarkdown(source)
	_ = parseMarkdown("[reference][r]\n\n[r]: https://different.test")
	if !reflect.DeepEqual(original, parseMarkdown(source)) {
		t.Fatal("parser state leaked across documents")
	}
	for _, source := range []string{"", "\n", "a\n", "a\n\nb", "中文\r\nlast"} {
		var got strings.Builder
		for i, line := range highlight(source, "") {
			if i > 0 {
				got.WriteByte('\n')
			}
			for _, span := range line {
				got.WriteString(span.Text)
			}
		}
		if got.String() != strings.ReplaceAll(source, "\r\n", "\n") {
			t.Fatalf("code line contents changed: %q != %q", got.String(), source)
		}
	}
}

func BenchmarkOptimization202(b *testing.B) {
	b.Run("MarkdownSmall", func(b *testing.B) {
		source := "### 标题\n\nParagraph with **bold** and a [link](https://example.test).\n\n- first\n- second"
		b.ReportAllocs()
		for b.Loop() {
			_ = parseMarkdown(source)
		}
	})
	b.Run("Highlight2000Lines", func(b *testing.B) {
		source := strings.Repeat("// 中文\nconst value = 123;\n", 1000)
		b.ReportAllocs()
		for b.Loop() {
			_ = highlight(source, "javascript")
		}
	})
}
