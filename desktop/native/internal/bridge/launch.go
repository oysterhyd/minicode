package bridge

import (
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
)

func Launch(project, resources, userData string) *exec.Cmd {
	env := environment("PYTHONIOENCODING=utf-8", "PYTHONUTF8=1")
	packaged := filepath.Join(resources, "runtime")
	if _, err := os.Stat(filepath.Join(packaged, "bridge.py")); err == nil {
		python := filepath.Join(packaged, "python")
		exe := filepath.Join(python, "python.exe")
		if runtime.GOOS != "windows" {
			exe = filepath.Join(python, "bin", "python3")
		}
		cmd := exec.Command(exe, "-I", "-X", "utf8", "-u", filepath.Join(packaged, "bridge.py"))
		cmd.Dir = userData
		cmd.Env = without(env, "PYTHONHOME", "PYTHONPATH", "PATH")
		cmd.Env = append(cmd.Env, "PATH="+strings.Join([]string{
			python, filepath.Join(python, "Scripts"), filepath.Join(packaged, "git", "cmd"), os.Getenv("PATH"),
		}, string(os.PathListSeparator)))
		hideWindow(cmd)
		return cmd
	}
	python := filepath.Join(project, ".venv", "Scripts", "python.exe")
	if runtime.GOOS != "windows" {
		python = filepath.Join(project, ".venv", "bin", "python")
	}
	if _, err := os.Stat(python); err != nil {
		python = os.Getenv("MINICODE_PYTHON")
		if python == "" {
			python = "python"
		}
	}
	cmd := exec.Command(python, "-u", filepath.Join(project, "desktop", "bridge.py"))
	cmd.Dir = project
	cmd.Env = append(without(env, "PYTHONPATH"), "PYTHONPATH="+filepath.Join(project, "src"))
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
