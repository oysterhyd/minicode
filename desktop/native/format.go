package main

import (
	"fmt"
	"path/filepath"
	"strconv"
	"strings"
	"time"
)

func fmtInt(n int) string         { return strconv.Itoa(n) }
func basename(path string) string { return filepath.Base(strings.ReplaceAll(path, `\`, "/")) }
func dirname(path string) string {
	return filepath.ToSlash(filepath.Dir(strings.ReplaceAll(path, `\`, "/")))
}
func relativeTime(value string) string {
	then, _ := time.Parse(time.RFC3339Nano, value)
	diff := time.Since(then)
	if diff < time.Minute {
		return "刚刚"
	}
	if diff < time.Hour {
		return fmt.Sprintf("%d 分钟", int(diff.Minutes()))
	}
	if diff < 24*time.Hour {
		return fmt.Sprintf("%d 小时", int(diff.Hours()))
	}
	if diff < 7*24*time.Hour {
		return fmt.Sprintf("%d 天", int(diff.Hours()/24))
	}
	return then.Format("01/02")
}
func number(n int) string {
	if n >= 1_000_000 {
		return strings.TrimSuffix(fmt.Sprintf("%.1f", float64(n)/1_000_000), ".0") + "M"
	}
	if n >= 1_000 {
		return strings.TrimSuffix(fmt.Sprintf("%.1f", float64(n)/1_000), ".0") + "K"
	}
	return strconv.Itoa(n)
}
