package main

import (
	"github.com/egoist/mygo"
	"github.com/egoist/mygo/ui"
	"minicode.desktop/internal/model"
	"path/filepath"
	"slices"
	"strings"
)

var mediaLabels = map[string]string{"image": "图片", "pdf": "PDF", "audio": "音频", "video": "视频"}

func isMediaPath(path string) bool {
	return slices.Contains([]string{".png", ".jpg", ".jpeg", ".gif", ".webp", ".pdf", ".wav", ".mp3", ".mp4", ".mov", ".webm"}, strings.ToLower(filepath.Ext(path)))
}

func capabilityLabel(c model.ModelCapabilities) string {
	var labels []string
	for _, kind := range []string{"image", "pdf", "audio", "video"} {
		if c.Supports(kind) {
			labels = append(labels, mediaLabels[kind])
		}
	}
	if len(labels) == 0 {
		return "文本"
	}
	return "文本 / " + strings.Join(labels, " / ")
}

func (d *desktop) attachmentProblem(v *conversation, files []model.Attachment) string {
	if len(files) == 0 {
		return ""
	}
	var caps model.ModelCapabilities
	for _, m := range d.models {
		if m.ID == v.State.Model {
			caps = m.InputCapabilities
		}
	}
	var unsupported []string
	total := 0
	for _, file := range files {
		total += file.Size
		if !caps.Supports(file.Kind) && !slices.Contains(unsupported, mediaLabels[file.Kind]) {
			unsupported = append(unsupported, mediaLabels[file.Kind])
		}
	}
	if len(files) > 10 || total > 24000000 {
		return "最多 10 个附件，总大小不能超过 24 MB"
	}
	if len(unsupported) > 0 {
		return "当前模型未启用 " + strings.Join(unsupported, "、") + " 输入能力，请确认模型配置或切换模型。"
	}
	return ""
}

func (d *desktop) chooseAttachments(v *conversation) {
	go func() {
		paths, err := mygo.Dialog.Open(mygo.OpenDialogOptions{Parent: d.window, Title: "添加附件", Multiple: true, Filters: []mygo.FileFilter{{Name: "图片、PDF、音频、视频", Extensions: []string{"png", "jpg", "jpeg", "gif", "webp", "pdf", "wav", "mp3", "mp4", "mov", "webm"}}}})
		d.schedule(func() {
			if err != nil {
				d.error(v, err)
			} else if len(paths) > 0 {
				d.addAttachments(v, paths)
			}
		})
	}()
}

func (d *desktop) addAttachments(v *conversation, paths []string) {
	if v.Workspace == "" {
		v.Error = "请先选择工作区"
		return
	}
	if v.Uploading {
		v.Error = "附件正在添加，请稍候"
		return
	}
	v.Uploading = true
	var added []model.Attachment
	d.request(v, "importAttachments", map[string]any{"paths": paths}, &added, func(err error) {
		v.Uploading = false
		if err != nil {
			d.error(v, err)
			return
		}
		files := slices.Clone(v.Attachments)
		for _, file := range added {
			if !slices.ContainsFunc(files, func(existing model.Attachment) bool { return existing.AssetID == file.AssetID }) {
				files = append(files, file)
			}
		}
		total := 0
		for _, file := range files {
			total += file.Size
		}
		if len(files) > 10 || total > 24000000 {
			v.Error = "最多 10 个附件，总大小不能超过 24 MB"
			return
		}
		v.Attachments = files
		v.Error = ""
		d.focusPrompt = d.active == v.Key
	})
}

func (d *desktop) openAttachment(file model.Attachment) {
	v := d.current()
	var result struct {
		Path string `json:"path"`
	}
	d.request(v, "attachmentPath", map[string]any{"assetId": file.AssetID}, &result, func(err error) {
		if err != nil {
			d.error(v, err)
			return
		}
		go func() {
			if err := mygo.Shell.OpenPath(result.Path); err != nil {
				d.schedule(func() { d.error(v, err) })
			}
		}()
	})
}

func (d *desktop) attachmentChips(c *ui.Context, p palette, files []model.Attachment, editable bool) {
	row := ui.Row(c).FillWidth().Wrap().Gap(6)
	if !editable {
		row.MaxWidthPercent(85).Justify(ui.End)
	}
	row.Children(func() {
		for _, file := range files {
			ui.Row(c).Key(file.AssetID).Height(28).Gap(2).Radius(7).Border(1, p.Border).Background(p.Panel).Children(func() {
				b := ui.ButtonBase(c).Label("打开附件 "+file.Name).Description(mediaLabels[file.Kind]).Height(26).Padding(0, 8).Gap(6)
				b.Children(func() {
					icon(c, "FileText", 13, p.Text3)
					ui.Text(c, file.Name).SingleLine().MaxWidth(170).FontSize(p.font(11.2)).TextColor(p.Text2)
				})
				b.Tooltip(file.Name + " · " + mediaLabels[file.Kind])
				if b.Clicked() {
					d.openAttachment(file)
				}
				if editable && iconButton(c, p, "X", "移除附件 "+file.Name).Size(24, 24).Clicked() {
					v := d.current()
					v.Attachments = slices.DeleteFunc(v.Attachments, func(item model.Attachment) bool { return item.AssetID == file.AssetID })
				}
			})
		}
	})
}
