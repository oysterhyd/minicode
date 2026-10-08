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
	"testing"
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
