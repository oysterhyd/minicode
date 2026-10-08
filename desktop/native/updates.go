package main

import (
	"time"

	"github.com/egoist/mygo/plugins/updater"
	"github.com/egoist/mygo/ui"
)

type updateService interface {
	Enabled() bool
	AutomaticChecks() bool
	AutomaticDownloads() bool
	SetAutomaticChecks(bool)
	SetAutomaticDownloads(bool)
	LastCheck() time.Time
	Check()
}

type nativeUpdates struct{ enabled bool }

func (u nativeUpdates) Enabled() bool               { return u.enabled }
func (nativeUpdates) AutomaticChecks() bool         { return updater.AutomaticChecks() }
func (nativeUpdates) AutomaticDownloads() bool      { return updater.AutomaticDownloads() }
func (nativeUpdates) SetAutomaticChecks(on bool)    { updater.SetAutomaticChecks(on) }
func (nativeUpdates) SetAutomaticDownloads(on bool) { updater.SetAutomaticDownloads(on) }
func (nativeUpdates) LastCheck() time.Time          { return updater.LastCheck() }
func (nativeUpdates) Check()                        { updater.CheckForUpdates() }

func (d *desktop) updateSettings(c *ui.Context, p palette) {
	u := d.updates
	if u == nil {
		return
	}
	section(c, p, "应用更新", "从 oysterhyd/minicode 的 GitHub Releases 获取签名更新。", func() {
		settingsRow(c, p, "自动检查更新", "启动后在后台检查，每天最多一次。", func() {
			on := u.AutomaticChecks()
			if toggle(c, p, &on, "启用自动检查更新").Disabled(!u.Enabled()).Changed() {
				u.SetAutomaticChecks(on)
			}
		})
		settingsRow(c, p, "自动下载并安装", "新版本在下次启动时启用，不自动打断当前任务。", func() {
			on := u.AutomaticDownloads()
			if toggle(c, p, &on, "启用自动下载并安装").Disabled(!u.Enabled()).Changed() {
				u.SetAutomaticDownloads(on)
			}
		})
		d.checkUpdateButton(c, p)
	})
}

func (d *desktop) checkUpdateButton(c *ui.Context, p palette) {
	u := d.updates
	if u == nil {
		return
	}
	if button(c, p, "检查更新", "RefreshCw", false).Disabled(!u.Enabled()).Clicked() {
		u.Check()
	}
	if !u.Enabled() {
		muted(c, p, "此构建未启用更新，或安装目录不可写。请使用安装版并安装到当前用户可写的目录。").FontSize(p.font(10.92))
	} else if checked := u.LastCheck(); !checked.IsZero() {
		muted(c, p, "上次检查："+checked.Local().Format("2006-01-02 15:04")).FontSize(p.font(10.92))
	}
}
