package bridge

import (
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
)

func Launch(project, resources, userData string) *exec.Cmd {
	env := os.Environ()
	packaged := filepath.Join(resources, "runtime")
	name := "minicode"
	if runtime.GOOS == "windows" {
		name += ".exe"
	}
	if _, err := os.Stat(filepath.Join(packaged, name)); err == nil {
		cmd := exec.Command(filepath.Join(packaged, name), "bridge")
		cmd.Dir = userData
		cmd.Env = without(env, "PATH")
		cmd.Env = append(cmd.Env, "PATH="+strings.Join([]string{
			filepath.Join(packaged, "git", "cmd"), filepath.Join(packaged, "node"), os.Getenv("PATH"),
		}, string(os.PathListSeparator)))
		hideWindow(cmd)
		return cmd
	}
	exe := os.Getenv("MINICODE_AGENT")
	if exe == "" {
		exe = filepath.Join(project, "target", "release", name)
		if _, err := os.Stat(exe); err != nil {
			exe = filepath.Join(project, "target", "debug", name)
		}
	}
	cmd := exec.Command(exe, "bridge")
	cmd.Dir = project
	cmd.Env = env
	hideWindow(cmd)
	return cmd
}

func environment(values ...string) []string {
	keys := make([]string, len(values))
	for i, value := range values {
		keys[i], _, _ = strings.Cut(value, "=")
	}
	return append(without(os.Environ(), keys...), values...)
}

func without(env []string, keys ...string) []string {
	result := make([]string, 0, len(env))
	for _, value := range env {
		key, _, _ := strings.Cut(value, "=")
		skip := false
		for _, excluded := range keys {
			if strings.EqualFold(key, excluded) {
				skip = true
				break
			}
		}
		if !skip {
			result = append(result, value)
		}
	}
	return result
}
