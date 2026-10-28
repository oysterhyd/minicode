package main

import (
	"encoding/json"
	"fmt"
	"strings"

	"github.com/alecthomas/chroma/v2"
	"github.com/alecthomas/chroma/v2/lexers"
	"github.com/egoist/mygo/ui"
	"github.com/yuin/goldmark"
	"github.com/yuin/goldmark/ast"
	"github.com/yuin/goldmark/extension"
	east "github.com/yuin/goldmark/extension/ast"
	"github.com/yuin/goldmark/text"
	"minicode.desktop/internal/content"
	"minicode.desktop/internal/model"
)

type inline struct {
	Text, URL                  string
	Bold, Italic, Code, Strike bool
	Image                      bool
}
type docBlock struct {
	Kind, Text, Language string
	Level, Indent        int
	Spans                []inline
	Code                 []codeLine
	Table                [][][]inline
	ImageURL             string
}
type document struct{ Blocks []docBlock }
type codeSpan struct {
	Text string
	Type chroma.TokenType
}
type codeLine []codeSpan
type modelDiffLine = content.DiffLine

type renderCache struct {
	docs  map[string]*document
	order []string
	bytes int
	diffs map[string]cachedDiff
}

type cachedDiff struct {
	Before, After, Output string
	Lines                 []content.DiffLine
}

func newRenderCache() *renderCache {
	return &renderCache{docs: map[string]*document{}, diffs: map[string]cachedDiff{}}
}
func (cache *renderCache) get(source string) *document {
	if doc := cache.docs[source]; doc != nil {
		return doc
	}
	source = strings.Clone(source) // Chunk slices must not pin old stream buffers.
	doc := parseMarkdown(source)
	cache.store(source, doc)
	return doc
}

func (cache *renderCache) store(source string, doc *document) {
	cache.docs[source] = doc
	cache.order = append(cache.order, source)
	cache.bytes += len(source)
	for len(cache.order) > 1 && (len(cache.order) > 512 || cache.bytes > 8<<20) {
		cache.bytes -= len(cache.order[0])
		delete(cache.docs, cache.order[0])
		cache.order[0] = ""
		cache.order = cache.order[1:]
	}
}

func (cache *renderCache) markdown(source string) *document {
	if doc := cache.docs[source]; doc != nil {
		return doc
	}
	chunks := markdownChunks(source)
	if len(chunks) == 1 {
		return cache.get(chunks[0])
	}
	doc := &document{}
	for _, chunk := range chunks {
		doc.Blocks = append(doc.Blocks, cache.get(chunk).Blocks...)
	}
	cache.store(source, doc)
	return doc
}

func highlight(source, language string) []codeLine {
	lexer := lexers.Get(strings.TrimPrefix(language, "."))
	if lexer == nil {
		lexer = lexers.Match("source" + language)
	}
	if lexer == nil {
		lexer = lexers.Fallback
	}
	it, err := lexer.Tokenise(nil, source)
	if err != nil {
		it = chroma.Literator(chroma.Token{Type: chroma.Text, Value: source})
	}
	lines := []codeLine{{}}
	for token := it(); token != chroma.EOF; token = it() {
		first := true
		for part := range strings.SplitSeq(token.Value, "\n") {
			if !first {
				lines = append(lines, codeLine{})
			}
			first = false
			if part != "" {
				lines[len(lines)-1] = append(lines[len(lines)-1], codeSpan{part, token.Type})
			}
		}
	}
	return lines
}

// tokenColor colors code in Claude's quiet scheme: clay keywords, slate
// functions, sage strings and ochre numbers on the paper code block.
func tokenColor(kind chroma.TokenType, p palette) ui.Color {
	pair := func(light, dark string) ui.Color {
		if p.Dark {
			return ui.Hex(dark)
		}
		return ui.Hex(light)
	}
	switch {
	case kind.InCategory(chroma.Comment):
		return pair("#9c9a92", "#6e6c66")
	case kind == chroma.KeywordType || kind == chroma.NameClass:
		return pair("#7a5c99", "#c3a8de")
	case kind.InCategory(chroma.Keyword):
		return pair("#8f4a2e", "#e3a383")
	case kind.InCategory(chroma.LiteralString):
		return pair("#5c7a3e", "#a9c08f")
	case kind.InCategory(chroma.LiteralNumber):
		return pair("#a8742a", "#d7a85c")
	case kind == chroma.NameFunction || kind == chroma.NameBuiltin:
		return pair("#3b5f8a", "#9db7da")
	case kind == chroma.NameTag:
		return pair("#8f4a2e", "#e3a383")
	case kind == chroma.NameAttribute || kind.InSubCategory(chroma.NameVariable) || kind == chroma.NameProperty:
		return pair("#7d6230", "#d9c08f")
	}
	return p.Text
}

func codeText(c *ui.Context, p palette, line codeLine) {
	spans := make([]ui.Span, len(line))
	for i, span := range line {
		spans[i] = ui.Span{Text: span.Text, Color: tokenColor(span.Type, p), Italic: span.Type.InCategory(chroma.Comment)}
	}
	if len(spans) == 0 {
		spans = []ui.Span{{Text: " "}}
	}
	ui.RichText(c, spans...).Font(monoFont).FontSize(p.font(12.04)).NoWrap().Selectable().LineHeight(1.65)
}

var markdownParser = goldmark.New(goldmark.WithExtensions(extension.GFM)).Parser()

func parseMarkdown(source string) *document {
	data := []byte(source)
	root := markdownParser.Parse(text.NewReader(data))
	doc := &document{}
	var blocks func(ast.Node, int)
	blocks = func(parent ast.Node, indent int) {
		for n := parent.FirstChild(); n != nil; n = n.NextSibling() {
			switch node := n.(type) {
			case *ast.Heading:
				doc.Blocks = append(doc.Blocks, docBlock{Kind: "heading", Level: node.Level, Spans: inlines(n, data, inline{})})
			case *ast.Paragraph, *ast.TextBlock:
				var spans []inline
				for _, span := range inlines(n, data, inline{}) {
					if span.Image {
						if len(spans) > 0 {
							doc.Blocks = append(doc.Blocks, docBlock{Kind: "paragraph", Indent: indent, Spans: spans})
							spans = nil
						}
						doc.Blocks = append(doc.Blocks, docBlock{Kind: "image", Indent: indent, ImageURL: span.URL, Text: span.Text})
					} else {
						spans = append(spans, span)
					}
				}
				if len(spans) > 0 {
					doc.Blocks = append(doc.Blocks, docBlock{Kind: "paragraph", Indent: indent, Spans: spans})
				}
			case *ast.FencedCodeBlock:
				code := string(n.Lines().Value(data))
				language := string(node.Language(data))
				doc.Blocks = append(doc.Blocks, docBlock{Kind: "code", Text: strings.TrimSuffix(code, "\n"), Language: language, Code: highlight(strings.TrimSuffix(code, "\n"), language), Indent: indent})
			case *ast.CodeBlock:
				code := string(n.Lines().Value(data))
				doc.Blocks = append(doc.Blocks, docBlock{Kind: "code", Text: code, Code: highlight(code, ""), Indent: indent})
			case *ast.List:
				index := node.Start
				if index == 0 {
					index = 1
				}
				for child := n.FirstChild(); child != nil; child = child.NextSibling() {
					prefix := "• "
					if node.IsOrdered() {
						prefix = fmt.Sprintf("%d. ", index)
						index++
					}
					first := true
					for paragraph := child.FirstChild(); paragraph != nil; {
						next := paragraph.NextSibling()
						if paragraph.Kind() == ast.KindList {
							wrapper := ast.NewDocument()
							child.RemoveChild(child, paragraph)
							wrapper.AppendChild(wrapper, paragraph)
							blocks(wrapper, indent+1)
							paragraph = next
							continue
						}
						spans := inlines(paragraph, data, inline{})
						if first {
							spans = append([]inline{{Text: prefix}}, spans...)
							first = false
						}
						doc.Blocks = append(doc.Blocks, docBlock{Kind: "list", Indent: indent, Spans: spans})
						paragraph = next
					}
				}
			case *ast.Blockquote:
				start := len(doc.Blocks)
				blocks(n, indent)
				for i := start; i < len(doc.Blocks); i++ {
					if doc.Blocks[i].Kind == "paragraph" {
						doc.Blocks[i].Kind = "quote"
					}
				}
			case *east.Table:
				var rows [][][]inline
				for row := n.FirstChild(); row != nil; row = row.NextSibling() {
					var cells [][]inline
					for cell := row.FirstChild(); cell != nil; cell = cell.NextSibling() {
						cells = append(cells, inlines(cell, data, inline{}))
					}
					rows = append(rows, cells)
				}
				doc.Blocks = append(doc.Blocks, docBlock{Kind: "table", Table: rows})
			case *ast.ThematicBreak:
				doc.Blocks = append(doc.Blocks, docBlock{Kind: "divider"})
			default:
				if n.HasChildren() {
					blocks(n, indent)
				}
			}
		}
	}
	blocks(root, 0)
	return doc
}

func inlines(parent ast.Node, data []byte, style inline) []inline {
	var spans []inline
	var walk func(ast.Node, inline)
	walk = func(n ast.Node, s inline) {
		switch node := n.(type) {
		case *ast.Text:
			s.Text = string(node.Value(data))
			if node.SoftLineBreak() || node.HardLineBreak() {
				s.Text += "\n"
			}
			spans = append(spans, s)
			return
		case *ast.String:
			s.Text = string(node.Value)
			spans = append(spans, s)
			return
		case *ast.Emphasis:
			if node.Level == 2 {
				s.Bold = true
			} else {
				s.Italic = true
			}
		case *ast.CodeSpan:
			s.Code = true
		case *ast.Link:
			s.URL = string(node.Destination)
		case *ast.AutoLink:
			s.Text = string(node.Label(data))
			s.URL = string(node.URL(data))
			spans = append(spans, s)
			return
		case *east.Strikethrough:
			s.Strike = true
		case *east.TaskCheckBox:
			if node.IsChecked {
				s.Text = "☑ "
			} else {
				s.Text = "☐ "
			}
			spans = append(spans, s)
			return
		case *ast.Image:
			s.Image = true
			s.Text = string(node.Text(data))
			s.URL = string(node.Destination)
			spans = append(spans, s)
			return
		}
		for child := n.FirstChild(); child != nil; child = child.NextSibling() {
			walk(child, s)
		}
	}
	for child := parent.FirstChild(); child != nil; child = child.NextSibling() {
		walk(child, style)
	}
	return spans
}

func inlineText(c *ui.Context, p palette, spans []inline, bold bool) ui.Element {
	e := ui.RichText(c).Selectable().LineHeight(1.72)
	e.Children(func() {
		for _, span := range spans {
			var part ui.Element
			if span.URL != "" && allowedExternal(span.URL) {
				part = ui.Link(c, span.Text, span.URL).TextColor(p.Text).Underline()
			} else {
				part = ui.Text(c, span.Text)
			}
			if span.Bold || bold {
				part.FontWeight(600)
			}
			if span.Italic {
				part.Italic()
			}
			if span.Strike {
				part.Strikethrough()
			}
			if span.Code {
				part.Font(monoFont).FontSize(p.font(12.32)).TextBackground(p.Active).TextColor(p.Text)
			}
		}
	})
	return e
}

func (d *desktop) markdown(c *ui.Context, p palette, source string) {
	doc := d.renderCache.markdown(source)
	// Replies are set as Claude sets them: a book face, a little larger,
	// with generous leading.
	ui.Column(c).Gap(12).Font(p.Prose).FontSize(p.font(14.7)).Children(func() {
		for i, block := range doc.Blocks {
			ui.Column(c.Key(i)).Margin(0, 0, 0, float32(block.Indent)*18).Children(func() {
				switch block.Kind {
				case "image":
					d.markdownImage(c, p, block.ImageURL, block.Text)
				case "heading":
					size := float32(15.4)
					if block.Level == 1 {
						size = 20.3
					}
					if block.Level == 2 {
						size = 17.5
					}
					if block.Level >= 4 {
						size = 14.7
					}
					heading := inlineText(c, p, block.Spans, true).FontSize(p.font(size)).LineHeight(1.35)
					if i > 0 {
						heading.Margin(10, 0, 0, 0)
					}
				case "paragraph", "list":
					inlineText(c, p, block.Spans, false)
				case "quote":
					ui.Column(c).BorderWidth(0, 0, 0, 2).BorderColor(p.BorderStrong).Padding(0, 0, 0, 14).Children(func() { inlineText(c, p, block.Spans, false).TextColor(p.Text2) })
				case "divider":
					ui.Divider(c).Margin(4, 0)
				case "code":
					ui.Column(c).Margin(2, 0).Radius(14).Clip().Border(1, p.CodeBorder).Background(p.Code).Font(sansFont).Children(func() {
						ui.Row(c).Height(34).Padding(0, 6, 0, 14).Children(func() {
							language := block.Language
							if language == "" {
								language = "text"
							}
							ui.Text(c, language).Font(monoFont).FontSize(p.font(10.64)).TextColor(p.Text3)
							ui.Spacer(c)
							copyCode := button(c, p, tr("复制代码"), "Copy", false).Height(24).Padding(0, 8).Radius(8).FontSize(p.font(10.36)).Border(0, ui.Transparent)
							if copyCode.Clicked() {
								c.WriteClipboard(block.Text)
							}
						})
						ui.ScrollHorizontal(c).Padding(2, 14, 12, 14).Children(func() {
							ui.Column(c).Children(func() {
								for _, line := range block.Code {
									codeText(c, p, line)
								}
							})
						})
					})
				case "table":
					ui.Column(c).Border(1, p.Border).Radius(12).Clip().Font(sansFont).Children(func() {
						for rowIndex, row := range block.Table {
							ui.Row(c).AlignItems(ui.Stretch).Background(func() ui.Color {
								if rowIndex == 0 {
									return p.Subtle
								}
								return ui.Transparent
							}()).BorderWidth(0, 0, func() float32 {
								if rowIndex == len(block.Table)-1 {
									return 0
								}
								return 1
							}(), 0).BorderColor(p.BorderSubtle).Children(func() {
								for _, cell := range row {
									ui.Column(c).Grow(1).Basis(0).Padding(8, 12).Children(func() { inlineText(c, p, cell, rowIndex == 0).FontSize(p.font(12.6)) })
								}
							})
						}
					})
				}
			})
		}
	})
}

func convertDiff(text string) []modelDiffLine { return content.ParseDiff(text) }

func markdownChunks(source string) []string {
	var chunks []string
	start, position, fence, previous := 0, 0, "", ""
	for line := range strings.SplitSeq(source, "\n") {
		trim := strings.TrimSpace(line)
		if strings.HasPrefix(trim, "```") || strings.HasPrefix(trim, "~~~") {
			marker := trim[:3]
			if fence == "" {
				fence = marker
			} else if strings.HasPrefix(trim, fence) && strings.Trim(trim, "`~ ") == "" {
				fence = ""
			}
		}
		if fence == "" && trim == "" && position > start {
			if !strings.HasPrefix(previous, "- ") && !strings.HasPrefix(previous, "* ") && !strings.HasPrefix(previous, "+ ") {
				chunks = append(chunks, source[start:position-1])
				start = position + len(line) + 1
			}
		}
		previous = trim
		position += len(line) + 1
	}
	if start <= len(source) {
		chunks = append(chunks, source[start:])
	}
	if len(chunks) == 0 {
		return []string{source}
	}
	return chunks
}

func (d *desktop) itemDiff(item *model.FeedItem) []content.DiffLine {
	key := d.current().Key + ":" + item.ID
	before, after := model.String(item.Args["old_text"]), model.String(item.Args["new_text"])
	if item.Name == "edit" && item.Args["edits"] != nil {
		var edits []struct {
			Old string `json:"old_text"`
			New string `json:"new_text"`
		}
		if encoded, err := json.Marshal(item.Args["edits"]); err == nil && json.Unmarshal(encoded, &edits) == nil {
			var oldParts, newParts []string
			for i, edit := range edits {
				label := fmt.Sprintf("编辑 %d\n", i+1)
				oldParts = append(oldParts, label+edit.Old)
				newParts = append(newParts, label+edit.New)
			}
			before, after = strings.Join(oldParts, "\n"), strings.Join(newParts, "\n")
		}
	}
	if item.Name == "write" {
		after = model.String(item.Args["content"])
	}
	if cached, ok := d.renderCache.diffs[key]; ok && cached.Before == before && cached.After == after && cached.Output == item.Output {
		return cached.Lines
	}
	var lines []content.DiffLine
	if item.Success {
		parsed := content.ParseDiff(item.Output)
		for _, line := range parsed {
			if line.Type == "hunk" {
				for _, line := range parsed {
					if line.Type != "meta" {
						lines = append(lines, line)
					}
				}
				break
			}
		}
	}
	if lines == nil {
		lines = content.DiffTexts(before, after)
	}
	if len(d.renderCache.diffs) > 256 {
		clear(d.renderCache.diffs)
	}
	d.renderCache.diffs[key] = cachedDiff{before, after, item.Output, lines}
	return lines
}
