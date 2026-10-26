package bridge

import (
	"context"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"
)

func TestStderrProcess(t *testing.T) {
	if os.Getenv("MINICODE_STDERR_HELPER") != "1" {
		return
	}
	fmt.Fprintln(os.Stderr, "ordinary startup log: Error count=0 中文")
	fmt.Fprintln(os.Stderr, strings.Repeat("x", 2<<20))
	fmt.Fprintln(os.Stdout, `{"event":"log","text":"protocol diagnostic"}`)
	fmt.Fprintln(os.Stdout, `{"event":"warning","text":"optional feature unavailable"}`)
	fmt.Fprintln(os.Stdout, `{"id":1,"result":true}`)
	_, _ = io.Copy(io.Discard, os.Stdin)
	os.Exit(0)
}
func TestStderrIsDrainedToLogWithoutErrorEvents(t *testing.T) {
	logPath := filepath.Join(t.TempDir(), "agent.log")
	cmd := exec.Command(os.Args[0], "-test.run=^TestStderrProcess$")
	cmd.Env = append(os.Environ(), "MINICODE_STDERR_HELPER=1", "MINICODE_BRIDGE_LOG="+logPath)
	var mu sync.Mutex
	var events []Event
	c, err := Start(cmd, func(e Event) { mu.Lock(); events = append(events, e); mu.Unlock() })
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = c.Close() })
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	var result bool
	if err = c.Request(ctx, "initialize", nil, &result); err != nil {
		t.Fatal(err)
	}
	if !result {
		t.Fatal("reply was lost while draining stderr")
	}
	_ = c.Close()
	mu.Lock()
	defer mu.Unlock()
	warnings := 0
	for _, e := range events {
		if e.Event == "log" || (e.Event == "bridge_error" && !e.Fatal) {
			t.Fatalf("diagnostic leaked as an event: %+v", e)
		}
		if e.Event == "warning" {
			warnings++
		}
	}
	if warnings != 1 {
		t.Fatalf("warning not forwarded: %+v", events)
	}
	data, err := os.ReadFile(logPath)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(data), "ordinary startup log: Error count=0 中文") || !strings.Contains(string(data), "protocol diagnostic") || len(data) < 2<<20 {
		t.Fatal("diagnostic log incomplete")
	}
}
