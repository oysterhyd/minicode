package main

import (
	"bytes"
	"encoding/json"
	"fmt"
	"image"
	"image/draw"
	"image/png"
	"os"
	"path/filepath"
)

func load(file string) *image.NRGBA {
	f, err := os.Open(file)
	if err != nil {
		panic(err)
	}
	defer f.Close()
	img, err := png.Decode(f)
	if err != nil {
		panic(err)
	}
	converted := image.NewNRGBA(img.Bounds())
	draw.Draw(converted, converted.Bounds(), img, img.Bounds().Min, draw.Src)
	return converted
}

func main() {
	if len(os.Args) != 4 {
		panic("usage: compare-snapshots before after output.json")
	}
	files, err := filepath.Glob(filepath.Join(os.Args[1], "*-1x.png"))
	if err != nil || len(files) != 40 {
		panic("expected 40 baseline snapshots")
	}
	results := []map[string]any{}
	for _, file := range files {
		name := filepath.Base(file)
		before, after := load(file), load(filepath.Join(os.Args[2], name))
		if before.Bounds() != after.Bounds() {
			panic("snapshot dimensions changed: " + name)
		}
		changed := 0
		minX, minY, maxX, maxY := before.Bounds().Max.X, before.Bounds().Max.Y, -1, -1
		if !bytes.Equal(before.Pix, after.Pix) {
			for y := before.Bounds().Min.Y; y < before.Bounds().Max.Y; y++ {
				for x := before.Bounds().Min.X; x < before.Bounds().Max.X; x++ {
					if before.NRGBAAt(x, y) != after.NRGBAAt(x, y) {
						changed++
						minX = min(minX, x)
						minY = min(minY, y)
						maxX = max(maxX, x)
						maxY = max(maxY, y)
					}
				}
			}
		}
		results = append(results, map[string]any{"name": name, "changed_pixels": changed, "bounds": []int{minX, minY, maxX, maxY}, "identical": changed == 0})
	}
	data, err := json.MarshalIndent(results, "", "  ")
	if err != nil {
		panic(err)
	}
	if err = os.WriteFile(os.Args[3], append(data, '\n'), 0644); err != nil {
		panic(err)
	}
	identical := 0
	for _, r := range results {
		if r["identical"] == true {
			identical++
		}
	}
	fmt.Printf("%d/%d snapshots pixel-identical\n", identical, len(results))
}
