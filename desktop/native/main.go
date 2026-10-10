package main

import (
	"flag"
	"log"
	"os"
	"path/filepath"
	"sync"
	"time"

	"github.com/egoist/mygo"
	"github.com/egoist/mygo/plugins/updater"
	updaterui "github.com/egoist/mygo/plugins/updater/native"
	"github.com/egoist/mygo/ui"
	"minicode.desktop/internal/bridge"
	"minicode.desktop/internal/workspace"
)

const appVersion = "1.2.1"

func projectRoot() string {
	if override := os.Getenv("MINICODE_PROJECT_ROOT"); override != "" {
		return override
	}
	dir, _ := os.Getwd()
	for {
		if _, err := os.Stat(filepath.Join(dir, "desktop", "bridge.py")); err == nil {
			return dir
		}
		parent := filepath.Dir(dir)
		if parent == dir {
			break
		}
		dir = parent
	}
	exe, _ := os.Executable()
	return filepath.Dir(exe)
}

func main() {
	started := time.Now()
	fixture := flag.Bool("fixture", false, "run the isolated UI regression fixture")
	benchmark := flag.String("benchmark", "", "measure isolated native controller-to-view updates")
	capture := flag.String("capture", "", "capture the native window to a PNG and exit")
	probe := flag.String("probe", "", "verify the installed native window and Python bridge")
	fixturePage := flag.String("fixture-page", "home", "fixture state: home, conversation, approval, settings, files, diff")
	theme := flag.String("theme", "", "override appearance for regression checks")
	flag.Parse()
	if *benchmark != "" {
		*fixture = true
	}
	mygo.App.SetName("MiniCode")
	mygo.App.SetVersion(appVersion)
	if *fixture {
		dir, err := os.MkdirTemp("", "minicode-native-qa-")
		if err != nil {
			log.Fatal(err)
		}
		mygo.App.SetPath(mygo.PathUserData, dir)
	}
	if !mygo.App.RequestSingleInstanceLock() {
		return
	}
	if !*fixture {
		mygo.Use(updaterui.New(updater.Options{Language: "zh-Hans"}))
	}
	var d *desktop
	var client *bridge.Client
	mygo.App.OnSecondInstance(func(_ []string, _ string) {
		if d != nil && d.window != nil {
			d.window.Restore()
			d.window.Show()
			d.window.Focus()
		}
	})
	var quitting sync.Once
	ready := false
	mygo.App.OnBeforeQuit(func(e *mygo.QuitEvent) {
		if ready || d == nil {
			return
		}
		e.PreventDefault()
		quitting.Do(func() {
			d.closed.Store(true)
			go func() {
				if client != nil {
					_ = client.Close()
				}
				if d.store != nil {
					d.store.flush()
				}
				mygo.RunOnMain(func() { ready = true; mygo.App.Quit() })
			}()
		})
	})
	mygo.App.WhenReady(func() {
		dir, err := mygo.App.Path(mygo.PathUserData)
		if err != nil {
			log.Fatal(err)
		}
		resources, err := mygo.App.Path(mygo.PathResources)
		if err != nil {
			log.Fatal(err)
		}
		d = newDesktop(func(fn func()) {
			if d.window != nil {
				d.window.Update(fn)
			} else {
				mygo.RunOnMain(fn)
			}
		})
		d.loadPreferences(dir)
		if !*fixture {
			d.updates = nativeUpdates{enabled: mygo.Updater.Enabled()}
			updater.OnChange(func() { d.schedule(func() {}) })
		}
		if *theme != "" {
			d.preferences.Theme = *theme
		}
		source := mygo.ThemeSystem
		switch d.preferences.Theme {
		case "dark":
			source = mygo.ThemeDark
		case "light":
			source = mygo.ThemeLight
		}
		mygo.Theme.SetSource(source)
		if *fixture {
			loadFixture(d, *fixturePage)
		} else {
			cmd := bridge.Launch(projectRoot(), resources, dir)
			d.files = workspace.New(cmd.Env)
			client, err = bridge.Start(cmd, d.queueEvent)
			if err != nil {
				d.current().Error = err.Error()
			} else {
				d.client = client
			}
		}
		background := "#f5f5f3"
		if mygo.Theme.IsDark() {
			background = "#141416"
		}
		d.window = mygo.NewWindow(mygo.WindowOptions{
			Title: "MiniCode", Width: 1500, Height: 940, MinWidth: 1030, MinHeight: 680,
			StateKey: "main", TitleBarStyle: mygo.TitleBarHidden, TitleBarHeight: 40,
			AutoHideMenuBar: true, Content: ui.View(func(c *ui.Context) {
				d.view(c)
				if d.frameObserved != nil {
					d.frameObserved()
				}
			}),
			BackgroundColor: background,
		})
		d.notify = func(title, body, id string) {
			if d.window.IsFocused() {
				return
			}
			n := mygo.NewNotification(mygo.NotificationOptions{Title: title, Body: body, ID: id})
			n.OnClick(func() {
				d.window.Restore()
				d.window.Focus()
				for _, session := range d.sessions {
					if session.ID == id {
						d.openSession(session)
						break
					}
				}
			})
			_ = n.Show()
		}
		if !*fixture && d.client != nil {
			d.initialize()
		}
		if *probe != "" {
			go runInstalledProbe(d, *probe, started)
		}
		if *benchmark != "" {
			go runUIBenchmark(d, *benchmark)
		}
		if *capture != "" {
			path := *capture
			time.AfterFunc(700*time.Millisecond, func() {
				data, err := d.window.CapturePage()
				if err == nil {
					err = os.WriteFile(path, data, 0600)
				}
				if err != nil {
					log.Print("capture: ", err)
				}
				mygo.App.Quit()
			})
		}
	})
	if err := mygo.App.Run(); err != nil {
		log.Fatal(err)
	}
}
