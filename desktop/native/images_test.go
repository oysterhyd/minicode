package main

import (
	"bytes"
	"image"
	"image/color"
	"image/png"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/egoist/mygo/ui"
	"minicode.desktop/internal/model"
)

func TestMarkdownImagesLoadAndStayWithinWorkspace(t *testing.T) {
	root := t.TempDir()
	picture := image.NewRGBA(image.Rect(0, 0, 2, 3))
	picture.Set(0, 0, color.RGBA{R: 255, A: 255})
	var encoded bytes.Buffer
	if err := png.Encode(&encoded, picture); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "example.png"), encoded.Bytes(), 0600); err != nil {
		t.Fatal(err)
	}
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { _, _ = w.Write(encoded.Bytes()) }))
	defer server.Close()
	for _, address := range []string{"example.png", server.URL} {
		if _, err := loadMessageImage(root, address); err != nil {
			t.Fatal(err)
		}
	}
	if _, err := loadMessageImage(root, "../outside.png"); err == nil {
		t.Fatal("image escaped workspace")
	}
	doc := parseMarkdown("Before ![example](example.png) after")
	if len(doc.Blocks) != 3 || doc.Blocks[1].Kind != "image" || doc.Blocks[1].Text != "example" {
		t.Fatal("inline image order or alt text lost")
	}
}

func TestReadImageThumbnailBoundedAndAspectPreserved(t *testing.T) {
	path := filepath.Join(t.TempDir(), "read.png")
	picture := image.NewRGBA(image.Rect(0, 0, 1600, 800))
	for y := 0; y < 800; y++ {
		for x := 0; x < 1600; x++ {
			picture.SetRGBA(x, y, color.RGBA{R: 200, A: 255})
		}
	}
	var encoded bytes.Buffer
	if err := png.Encode(&encoded, picture); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, encoded.Bytes(), 0600); err != nil {
		t.Fatal(err)
	}
	bitmap, err := loadReadImage(path)
	if err != nil {
		t.Fatal(err)
	}
	w, h := bitmap.Size()
	if w != 320 || h != 160 {
		t.Fatalf("thumbnail dimensions %dx%d", w, h)
	}
	if w, h = thumbnailSize(200, 1200); w != 40 || h != 240 {
		t.Fatalf("portrait aspect ratio %dx%d", w, h)
	}
	if err := os.WriteFile(path, []byte("not an image"), 0600); err != nil {
		t.Fatal(err)
	}
	if _, err := loadReadImage(path); err == nil {
		t.Fatal("invalid bytes produced preview")
	}
}

func TestReadImageToolVisibleWhenOperationGroupCollapsed(t *testing.T) {
	d := newDesktop(func(fn func()) { fn() })
	asset := strings.Repeat("a", 64)
	picture := image.NewRGBA(image.Rect(0, 0, 120, 80))
	for y := 0; y < 80; y++ {
		for x := 0; x < 120; x++ {
			picture.SetRGBA(x, y, color.RGBA{R: 250, A: 255})
		}
	}
	d.images = map[string]*messageImage{"read:" + asset: {source: ui.NewBitmap(picture)}}
	item := &model.FeedItem{ID: "image-read", Kind: "tool", Name: "read", Success: true, Args: map[string]any{"path": "image.png"}, Attachments: []model.Attachment{{AssetID: asset, Kind: "image", Name: "image.png", MIME: "image/png", Size: 100}}}
	d.current().Open["group-image-read"] = false
	rendered := ui.Render(func(c *ui.Context) {
		c.Root().Background(ui.RGB(255, 255, 255))
		d.toolGroup(c, colors(false), model.FeedRow{ID: item.ID, Tools: []*model.FeedItem{item}})
	}, 400, 300, 1)
	red := 0
	for y := 0; y < 300; y++ {
		for x := 0; x < 400; x++ {
			r, g, b, _ := rendered.At(x, y).RGBA()
			if r > 50000 && g < 1000 && b < 1000 {
				red++
			}
		}
	}
	if red < 5000 {
		t.Fatalf("collapsed read-image preview absent: %d red pixels", red)
	}
	dir := filepath.Join("..", "..", "output", "native")
	if err := os.MkdirAll(dir, 0755); err != nil {
		t.Fatal(err)
	}
	file, err := os.Create(filepath.Join(dir, "read-image-preview-light.png"))
	if err != nil {
		t.Fatal(err)
	}
	defer file.Close()
	if err := png.Encode(file, rendered); err != nil {
		t.Fatal(err)
	}
}

func TestLiveImageResultAssociatesOnlySuccessfulTool(t *testing.T) {
	d := newDesktop(func(fn func()) { fn() })
	v := d.current()
	v.refreshPending = true // This test owns the event state; no asynchronous RPC.
	for _, id := range []string{"image-read", "failed-read"} {
		d.runtimeEvent(v, model.Event{Type: "tool_call_start", Data: map[string]any{"call_id": id, "name": "read", "arguments": map[string]any{"path": "image.png"}}})
	}
	image := map[string]any{"asset_id": strings.Repeat("b", 64), "kind": "image", "name": "image.png", "mime_type": "image/png", "size": 123}
	for i, id := range []string{"image-read", "failed-read"} {
		d.runtimeEvent(v, model.Event{Type: "tool_call_result", Data: map[string]any{"call_id": id, "success": i == 0, "images": []any{image}}})
	}
	if len(v.Items) != 2 || len(v.Items[0].Attachments) != 1 || len(v.Items[1].Attachments) != 0 {
		t.Fatal("live read references detached or displayed for failed tool")
	}
}
