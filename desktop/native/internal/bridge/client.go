package bridge

import (
	"bufio"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os/exec"
	"sync"
	"sync/atomic"
	"time"
)

type Event struct {
	Event      string          `json:"event"`
	ClientKey  string          `json:"clientKey"`
	SessionID  string          `json:"sessionId"`
	Text       string          `json:"text"`
	ApprovalID string          `json:"approvalId"`
	Error      string          `json:"error"`
	Item       json.RawMessage `json:"item"`
	Request    json.RawMessage `json:"request"`
	Result     json.RawMessage `json:"result"`
}

type reply struct {
	ID     uint64          `json:"id"`
	Result json.RawMessage `json:"result"`
	Error  string          `json:"error"`
}

type Client struct {
	cmd     *exec.Cmd
	input   io.WriteCloser
	writer  sync.Mutex
	mu      sync.Mutex
	pending map[uint64]chan reply
	next    atomic.Uint64
	closed  chan struct{}
	done    chan struct{}
	err     error
	onEvent func(Event)
}

func Start(cmd *exec.Cmd, onEvent func(Event)) (*Client, error) {
	input, err := cmd.StdinPipe()
	if err != nil {
		return nil, err
	}
	output, err := cmd.StdoutPipe()
	if err != nil {
		return nil, err
	}
	stderr, err := cmd.StderrPipe()
	if err != nil {
		return nil, err
	}
	c := &Client{cmd: cmd, input: input, pending: make(map[uint64]chan reply),
		closed: make(chan struct{}), done: make(chan struct{}), onEvent: onEvent}
	if err := cmd.Start(); err != nil {
		return nil, err
	}
	outputDone := make(chan struct{})
	stderrDone := make(chan struct{})
	go func() { defer close(outputDone); c.read(output) }()
	go func() {
		defer close(stderrDone)
		scanner := bufio.NewScanner(stderr)
		scanner.Buffer(make([]byte, 4096), 1<<20)
		for scanner.Scan() {
			c.emit(Event{Event: "bridge_error", Error: scanner.Text()})
		}
	}()
	go func() {
		<-outputDone
		<-stderrDone
		err := cmd.Wait()
		if err == nil {
			err = errors.New("Agent bridge exited")
		}
		c.fail(err)
		close(c.done)
	}()
	return c, nil
}

func (c *Client) emit(event Event) {
	if c.onEvent != nil {
		c.onEvent(event)
	}
}

func (c *Client) read(reader io.Reader) {
	scanner := bufio.NewScanner(reader)
	scanner.Buffer(make([]byte, 64<<10), 64<<20)
	for scanner.Scan() {
		line := scanner.Bytes()
		var result reply
		if err := json.Unmarshal(line, &result); err != nil {
			c.fail(fmt.Errorf("Agent bridge returned invalid JSON: %w", err))
			return
		}
		if result.ID != 0 {
			c.mu.Lock()
			waiter := c.pending[result.ID]
			delete(c.pending, result.ID)
			c.mu.Unlock()
			if waiter != nil {
				waiter <- result
			}
		} else {
			var event Event
			if err := json.Unmarshal(line, &event); err == nil {
				c.emit(event)
			}
		}
	}
	if err := scanner.Err(); err != nil {
		c.fail(fmt.Errorf("Agent bridge stream: %w", err))
	}
}

func (c *Client) fail(err error) {
	c.mu.Lock()
	if c.err != nil {
		c.mu.Unlock()
		return
	}
	c.err = err
	close(c.closed)
	for id, waiter := range c.pending {
		waiter <- reply{ID: id, Error: err.Error()}
		delete(c.pending, id)
	}
	c.mu.Unlock()
	c.emit(Event{Event: "bridge_error", Error: err.Error()})
}

func (c *Client) Request(ctx context.Context, method string, params any, target any) error {
	id := c.next.Add(1)
	if params == nil {
		params = map[string]any{}
	}
	data, err := json.Marshal(struct {
		ID     uint64 `json:"id"`
		Method string `json:"method"`
		Params any    `json:"params"`
	}{id, method, params})
	if err != nil {
		return err
	}
	if len(data) > 4_000_000 {
		return errors.New("request exceeds the size limit")
	}
	waiter := make(chan reply, 1)
	c.mu.Lock()
	if c.err != nil {
		err = c.err
		c.mu.Unlock()
		return err
	}
	c.pending[id] = waiter
	c.mu.Unlock()
	defer func() {
		c.mu.Lock()
		delete(c.pending, id)
		c.mu.Unlock()
	}()
	c.writer.Lock()
	_, err = c.input.Write(append(data, '\n'))
	c.writer.Unlock()
	if err != nil {
		return err
	}
	select {
	case result := <-waiter:
		if result.Error != "" {
			return errors.New(result.Error)
		}
		if target != nil {
			return json.Unmarshal(result.Result, target)
		}
		return nil
	case <-ctx.Done():
		return ctx.Err()
	case <-c.closed:
		c.mu.Lock()
		err := c.err
		c.mu.Unlock()
		return err
	}
}

func (c *Client) Close() error {
	c.writer.Lock()
	err := c.input.Close()
	c.writer.Unlock()
	select {
	case <-c.done:
	case <-time.After(3 * time.Second):
		_ = c.cmd.Process.Kill()
		<-c.done
	}
	return err
}
