package main

import (
	"image/png"
	"log"
	"os"
	"path/filepath"

	"github.com/egoist/mygo/ui"
)

func main() {
	if len(os.Args) != 3 {
		log.Fatal("usage: icon input.svg output.png")
	}
	data, err := os.ReadFile(os.Args[1])
	if err != nil {
		log.Fatal(err)
	}
	svg, err := ui.ParseSVG(data)
	if err != nil {
		log.Fatal(err)
	}
	image := ui.Render(func(c *ui.Context) { c.Root().Background(ui.Transparent); ui.Image(c, svg).Size(1024, 1024) }, 1024, 1024, 1)
	if err := os.MkdirAll(filepath.Dir(os.Args[2]), 0755); err != nil {
		log.Fatal(err)
	}
	file, err := os.Create(os.Args[2])
	if err != nil {
		log.Fatal(err)
	}
	defer file.Close()
	if err := png.Encode(file, image); err != nil {
		log.Fatal(err)
	}
}
