package main

import (
	"bytes"
	"context"
	"fmt"
	"image"
	"image/color"
	"io"
	"net/http"
	"net/url"
	"os"
	"strings"
	"time"

	"github.com/egoist/mygo/ui"
	"minicode.desktop/internal/model"
	"minicode.desktop/internal/workspace"
)

type messageImage struct {
	source  ui.ImageSource
	loading bool
	err     error
}

// Cached image references originate from an actual successful read tool call.
// Decode away from the UI thread, and cache only a bounded thumbnail.
func (d *desktop) readImages(c *ui.Context, p palette, files []model.Attachment) {
	ui.Row(c).Wrap().Gap(10).Children(func() {
		for _, file := range files {
			if file.Kind != "image" {
				continue
			}
			key := "read:" + file.AssetID
			if d.images == nil {
				d.images = map[string]*messageImage{}
			}
			entry := d.images[key]
			if entry == nil {
				if len(d.images) >= 64 {
					for previous, image := range d.images {
						if !image.loading {
							delete(d.images, previous)
							break
						}
					}
				}
				if len(d.images) >= 64 {
					continue
				}
				entry = &messageImage{loading: true}
				d.images[key] = entry
				var result struct {
					Path string `json:"path"`
				}
				d.request(d.current(), "attachmentPath", map[string]any{"assetId": file.AssetID}, &result, func(err error) {
					if err != nil {
						entry.err, entry.loading = err, false
						return
					}
					go func() {
						bitmap, err := loadReadImage(result.Path)
						d.schedule(func() { entry.source, entry.err, entry.loading = bitmap, err, false })
					}()
				})
			}
			ui.Column(c).Key(file.AssetID).MaxWidthPercent(100).Gap(4).Children(func() {
				b := ui.ButtonBase(c).Label(tr("打开图片 ")+file.Name).MaxWidthPercent(100).Radius(8).Border(1, p.Border).Background(p.Panel)
				b.Children(func() {
					if bitmap, ok := entry.source.(*ui.Bitmap); ok {
						width, height := bitmap.Size()
						ui.Image(c, bitmap).Size(float32(width), float32(height)).MaxWidthPercent(100).AspectRatio(float32(width) / float32(height)).Fit(ui.Contain).Label(file.Name)
					} else if entry.loading {
						muted(c, p, tr("加载图片…")).Padding(12)
					} else {
						muted(c, p, tr("图片无法预览")).Padding(12).Tooltip(fmt.Sprint(entry.err))
					}
				})
				if b.Clicked() {
					d.openAttachment(file)
				}
				muted(c, p, file.Name).SingleLine().MaxWidth(320).FontSize(p.font(10.5))
			})
		}
	})
}

func loadReadImage(path string) (*ui.Bitmap, error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	defer f.Close()
	data, err := io.ReadAll(io.LimitReader(f, 20_000_001))
	if err != nil {
		return nil, err
	}
	if len(data) > 20_000_000 {
		return nil, fmt.Errorf("image exceeds 20 MB")
	}
	config, _, err := image.DecodeConfig(bytes.NewReader(data))
	if err != nil {
		return nil, err
	}
	if config.Width <= 0 || config.Height <= 0 || config.Width > 8192 || config.Height > 8192 || config.Width*config.Height > 16_000_000 {
		return nil, fmt.Errorf("image dimensions exceed preview limit")
	}
	decoded, _, err := image.Decode(bytes.NewReader(data))
	if err != nil {
		return nil, err
	}
	w, h := thumbnailSize(config.Width, config.Height)
	thumbnail := image.NewRGBA(image.Rect(0, 0, w, h))
	// Bilinear resampling keeps the cached preview bounded without adding a
	// decoder dependency or retaining the large original bitmap in the feed.
	for y := 0; y < h; y++ {
		for x := 0; x < w; x++ {
			thumbnail.SetRGBA(x, y, thumbnailPixel(decoded, x, y, w, h))
		}
	}
	return ui.NewBitmap(thumbnail), nil
}

func thumbnailPixel(source image.Image, x, y, width, height int) color.RGBA {
	bounds := source.Bounds()
	fx := float64(x) * float64(max(0, bounds.Dx()-1)) / float64(max(1, width-1))
	fy := float64(y) * float64(max(0, bounds.Dy()-1)) / float64(max(1, height-1))
	x0, y0 := int(fx), int(fy)
	x1, y1 := min(x0+1, bounds.Dx()-1), min(y0+1, bounds.Dy()-1)
	rx, ry := fx-float64(x0), fy-float64(y0)
	var channels [4]float64
	for _, sample := range []struct {
		x, y   int
		weight float64
	}{{x0, y0, (1 - rx) * (1 - ry)}, {x1, y0, rx * (1 - ry)}, {x0, y1, (1 - rx) * ry}, {x1, y1, rx * ry}} {
		r, g, b, a := imageChannels(source, bounds.Min.X+sample.x, bounds.Min.Y+sample.y)
		for i, value := range []uint32{r, g, b, a} {
			channels[i] += float64(value) * sample.weight / 257
		}
	}
	return color.RGBA{uint8(channels[0] + 0.5), uint8(channels[1] + 0.5), uint8(channels[2] + 0.5), uint8(channels[3] + 0.5)}
}

// Concrete color values avoid boxing four samples per thumbnail pixel. Keep
// their RGBA conversion so alpha premultiplication and rounding stay identical.
func imageChannels(source image.Image, x, y int) (r, g, b, a uint32) {
	switch source := source.(type) {
	case *image.NRGBA:
		return source.NRGBAAt(x, y).RGBA()
	case *image.RGBA:
		return source.RGBAAt(x, y).RGBA()
	case *image.YCbCr:
		return source.YCbCrAt(x, y).RGBA()
	default:
		return source.At(x, y).RGBA()
	}
}

func thumbnailSize(width, height int) (int, int) {
	scale := min(1.0, 320.0/float64(width), 240.0/float64(height))
	return max(1, int(float64(width)*scale)), max(1, int(float64(height)*scale))
}

func (d *desktop) markdownImage(c *ui.Context, p palette, address, alt string) {
	root := d.current().Workspace
	key := root + ":" + address
	if d.images == nil {
		d.images = map[string]*messageImage{}
	}
	entry := d.images[key]
	if entry == nil {
		if len(d.images) >= 64 {
			if allowedExternal(address) {
				ui.Link(c, alt, address).TextColor(p.AccentText)
			} else {
				muted(c, p, alt)
			}
			return
		}
		entry = &messageImage{loading: true}
		d.images[key] = entry
		go func() {
			source, err := loadMessageImage(root, address)
			d.schedule(func() { entry.source, entry.err, entry.loading = source, err, false })
		}()
	}
	if entry.source != nil {
		ui.Image(c, entry.source).MaxWidthPercent(100).MaxHeight(640).Label(alt)
		return
	}
	if entry.loading {
		muted(c, p, "正在加载图片："+alt).Padding(10)
		return
	}
	ui.Text(c, alt).TextColor(p.Text3).Tooltip(fmt.Sprint(entry.err))
}

func loadMessageImage(root, address string) (ui.ImageSource, error) {
	u, err := url.Parse(address)
	if err != nil {
		return nil, err
	}
	var data []byte
	if u.Scheme == "http" || u.Scheme == "https" {
		ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
		defer cancel()
		request, err := http.NewRequestWithContext(ctx, http.MethodGet, address, nil)
		if err != nil {
			return nil, err
		}
		response, err := http.DefaultClient.Do(request)
		if err != nil {
			return nil, err
		}
		defer response.Body.Close()
		if response.StatusCode != 200 {
			return nil, fmt.Errorf("image HTTP %d", response.StatusCode)
		}
		data, err = io.ReadAll(io.LimitReader(response.Body, 16<<20+1))
		if err != nil {
			return nil, err
		}
	} else if u.Scheme == "" {
		path, err := workspace.Resolve(root, address)
		if err != nil {
			return nil, err
		}
		file, err := os.Open(path)
		if err != nil {
			return nil, err
		}
		defer file.Close()
		data, err = io.ReadAll(io.LimitReader(file, 16<<20+1))
		if err != nil {
			return nil, err
		}
	} else {
		return nil, fmt.Errorf("unsupported image scheme")
	}
	if len(data) > 16<<20 {
		return nil, fmt.Errorf("image exceeds 16 MiB")
	}
	if strings.Contains(string(data[:min(len(data), 512)]), "<svg") {
		return ui.ParseSVG(data)
	}
	config, _, err := image.DecodeConfig(bytes.NewReader(data))
	if err != nil {
		return nil, err
	}
	if config.Width > 8192 || config.Height > 8192 || config.Width*config.Height > 16_000_000 {
		return nil, fmt.Errorf("image dimensions exceed preview limit")
	}
	return ui.DecodeBitmap(data)
}
