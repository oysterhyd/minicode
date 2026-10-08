package main

import (
	"errors"
	"net/url"
	"path/filepath"
	"strings"

	"github.com/egoist/mygo"
	"minicode.desktop/internal/workspace"
)

func resolveWorkspace(root, path string) (string, error) { return workspace.Resolve(root, path) }
func relativeWorkspace(root, path string) (string, error) {
	canonical, err := filepath.EvalSymlinks(root)
	if err != nil {
		return "", err
	}
	relative, err := filepath.Rel(canonical, path)
	if err != nil {
		return "", err
	}
	if relative == "." {
		return "", errors.New("请选择工作区中的文件")
	}
	return filepath.ToSlash(relative), nil
}

func (d *desktop) chooseWorkspace() {
	go func() {
		paths, err := mygo.Dialog.Open(mygo.OpenDialogOptions{Parent: d.window, Title: "选择工作区", Directory: true})
		d.schedule(func() {
			if err != nil {
				d.error(d.current(), err)
				return
			}
			if len(paths) == 0 {
				return
			}
			root, err := filepath.EvalSymlinks(paths[0])
			if err != nil {
				d.error(d.current(), err)
				return
			}
			d.rememberWorkspace(root)
			d.newSession(root)
		})
	}()
}

func (d *desktop) chooseAcceptance(s *settingsState) {
	go func() {
		paths, err := mygo.Dialog.Open(mygo.OpenDialogOptions{Parent: d.window, Title: "选择验收配置", Filters: []mygo.FileFilter{{Name: "YAML acceptance", Extensions: []string{"yaml", "yml"}}}})
		d.schedule(func() {
			if err != nil {
				s.Error = err.Error()
			} else if len(paths) > 0 {
				s.Acceptance = paths[0]
			}
		})
	}()
}

func (d *desktop) reveal(root, path string) {
	go func() {
		target, err := workspace.Resolve(root, path)
		if err == nil {
			mygo.Shell.ShowItemInFolder(target)
		}
		if err != nil {
			d.schedule(func() { d.toast("无法打开文件", err.Error(), "error") })
		}
	}()
}
func (d *desktop) openPath(root, path string) {
	go func() {
		target, err := workspace.Resolve(root, path)
		if err == nil {
			err = mygo.Shell.OpenPath(target)
		}
		if err != nil {
			d.schedule(func() { d.toast("无法打开文件", err.Error(), "error") })
		}
	}()
}
func allowedExternal(value string) bool {
	u, err := url.Parse(value)
	if err != nil {
		return false
	}
	switch strings.ToLower(u.Scheme) {
	case "http", "https":
		return u.Host != ""
	case "mailto":
		return u.Opaque != ""
	}
	return false
}

func (d *desktop) applyAppearance() {
	if d.window == nil {
		return
	}
	source := mygo.ThemeSystem
	switch d.preferences.Theme {
	case "dark":
		source = mygo.ThemeDark
	case "light":
		source = mygo.ThemeLight
	}
	mygo.Theme.SetSource(source)
}
