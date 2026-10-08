//go:build !windows

package bridge

import "os/exec"

func hideWindow(cmd *exec.Cmd) {}
