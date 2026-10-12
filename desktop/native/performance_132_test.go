package main

import (
	"image"
	"image/color"
	"math/rand/v2"
	"testing"
)

type genericImage132 struct{ image.Image }

func TestThumbnail132PreservesEveryChannel(t *testing.T) {
	rng := rand.New(rand.NewPCG(132, 1))
	rect := image.Rect(7, 11, 73, 94)
	nrgba, rgba := image.NewNRGBA(rect), image.NewRGBA(rect)
	jpeg := image.NewYCbCr(rect, image.YCbCrSubsampleRatio420)
	for i := range nrgba.Pix {
		nrgba.Pix[i], rgba.Pix[i] = uint8(rng.Uint32()), uint8(rng.Uint32())
	}
	for _, data := range [][]byte{jpeg.Y, jpeg.Cb, jpeg.Cr} {
		for i := range data {
			data[i] = uint8(rng.Uint32())
		}
	}
	for _, source := range []image.Image{nrgba, rgba, jpeg, image.NewGray(rect)} {
		for _, size := range []image.Point{{1, 1}, {37, 41}, {120, 160}} {
			for y := range size.Y {
				for x := range size.X {
					got := thumbnailPixel(source, x, y, size.X, size.Y)
					want := thumbnailPixel(genericImage132{source}, x, y, size.X, size.Y)
					if got != want {
						t.Fatalf("%T pixel (%d,%d): %v != %v", source, x, y, got, want)
					}
				}
			}
		}
	}
}
func BenchmarkThumbnail132(b *testing.B) {
	source := image.NewNRGBA(image.Rect(0, 0, 1200, 800))
	for i := range source.Pix {
		source.Pix[i] = uint8(i * 31)
	}
	b.ReportAllocs()
	for b.Loop() {
		var pixel color.RGBA
		for y := range 213 {
			for x := range 320 {
				pixel = thumbnailPixel(source, x, y, 320, 213)
			}
		}
		if pixel.A == 0 {
			b.Fatal("unexpected transparent image")
		}
	}
}
