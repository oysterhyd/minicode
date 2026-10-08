//go:build !windows

package main

import "time"

var clockOrigin = time.Now()

func preciseMilliseconds() float64 { return float64(time.Since(clockOrigin).Nanoseconds()) / 1e6 }
