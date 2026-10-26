package bridge

import (
	"io"
	"strings"
	"testing"
)

type repeatedByte byte

func (b repeatedByte) Read(p []byte) (int, error) {
	for i := range p {
		p[i] = byte(b)
	}
	return len(p), nil
}
func TestStreamAcceptsAnEventLargerThan64MiB(t *testing.T) {
	const size = 64<<20 + 128
	waiter := make(chan reply, 1)
	seen := 0
	c := &Client{pending: map[uint64]chan reply{7: waiter}, closed: make(chan struct{}), onEvent: func(e Event) {
		if e.Event == "text_delta" {
			seen = len(e.Text)
		}
	}}
	c.read(io.MultiReader(strings.NewReader(`{"event":"text_delta","text":"`), io.LimitReader(repeatedByte('x'), size), strings.NewReader("\"}\n{\"id\":7,\"result\":true}\n")))
	if seen != size {
		t.Fatalf("large event was rejected: text=%d, err=%v", seen, c.err)
	}
	result := <-waiter
	if result.Error != "" || string(result.Result) != "true" {
		t.Fatalf("reply after large event lost: %+v", result)
	}
}
func TestStreamRejectsMalformedJSON(t *testing.T) {
	c := &Client{pending: map[uint64]chan reply{}, closed: make(chan struct{})}
	c.read(strings.NewReader("{invalid}\n"))
	if c.err == nil || !strings.Contains(c.err.Error(), "invalid JSON") {
		t.Fatalf("invalid stream accepted: %v", c.err)
	}
}
