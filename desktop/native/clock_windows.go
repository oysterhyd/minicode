package main

import (
	"syscall"
	"unsafe"
)

var performanceCounter = syscall.NewLazyDLL("kernel32.dll").NewProc("QueryPerformanceCounter")
var performanceFrequency = func() float64 {
	var n int64
	syscall.NewLazyDLL("kernel32.dll").NewProc("QueryPerformanceFrequency").Call(uintptr(unsafe.Pointer(&n)))
	return float64(n)
}()

func preciseMilliseconds() float64 {
	var n int64
	performanceCounter.Call(uintptr(unsafe.Pointer(&n)))
	return float64(n) * 1000 / performanceFrequency
}
