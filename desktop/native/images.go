package main

import (
	"bytes"
	"context"
	"fmt"
	"image"
	"io"
	"net/http"
	"net/url"
	"os"
	"strings"
	"time"

	"github.com/egoist/mygo/ui"
	"minicode.desktop/internal/workspace"
)

type messageImage struct {
	source  ui.ImageSource
	loading bool
	err     error
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
			if allowedExternal(address) { ui.Link(c, alt, address).TextColor(p.AccentText) } else { muted(c,p,alt) }
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
