package bridge

import (
	"bufio"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"sync/atomic"
	"time"
)

type Event struct {
	RunID      string          `json:"runId"`
	Event      string          `json:"event"`
	ClientKey  string          `json:"clientKey"`
	SessionID  string          `json:"sessionId"`
	Text       string          `json:"text"`
	ApprovalID string          `json:"approvalId"`
	Error      string          `json:"error"`
	Fatal      bool            `json:"fatal"`
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
	cmd         *exec.Cmd
	input       io.WriteCloser
	writer      sync.Mutex
	mu          sync.Mutex
	pending     map[uint64]chan reply
	next        atomic.Uint64
	closed      chan struct{}
	done        chan struct{}
	err         error
	onEvent     func(Event)
	diagnostics io.Writer
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
	logFile := diagnosticLog(cmd)
	c.diagnostics = io.Discard
	if logFile != nil {
		c.diagnostics = logFile
	}
	if err := cmd.Start(); err != nil {
		if logFile != nil {
			_ = logFile.Close()
		}
		return nil, err
	}
	outputDone := make(chan struct{})
	stderrDone := make(chan struct{})
	go func() { defer close(outputDone); c.read(output) }()
	go func() {
		defer close(stderrDone)
		// Copy chunks, not lines: long diagnostics must not stop draining
		// stderr and leave the agent blocked on a full pipe.
		_, _ = io.Copy(c.diagnostics, stderr)
	}()
	go func() {
		<-outputDone
		<-stderrDone
		err := cmd.Wait()
		if logFile != nil {
			_ = logFile.Close()
		}
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
	decoder := json.NewDecoder(bufio.NewReader(reader))
	for {
		var message struct {
			Event
			ID uint64 `json:"id"`
		}
		if err := decoder.Decode(&message); err != nil {
			if errors.Is(err, io.EOF) {
				c.fail(errors.New("Agent bridge output closed"))
			} else {
				c.fail(fmt.Errorf("Agent bridge returned invalid JSON: %w", err))
			}
			return
		}
		if message.ID != 0 {
			c.mu.Lock()
			waiter := c.pending[message.ID]
			delete(c.pending, message.ID)
			c.mu.Unlock()
			if waiter != nil {
				waiter <- reply{ID: message.ID, Result: message.Result, Error: message.Error}
			}
		} else if message.Event.Event == "log" {
			if c.diagnostics != nil {
				_, _ = fmt.Fprintln(c.diagnostics, "[log]", message.Text)
			}
		} else {
			c.emit(message.Event)
		}
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
	c.emit(Event{Event: "bridge_error", Error: err.Error(), Fatal: true})
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
	err := c.input.Close()
	select {
	case <-c.done:
	case <-time.After(3 * time.Second):
		_ = c.cmd.Process.Kill()
		<-c.done
	}
	return err
}

// Resolve against the child environment so profiles and packaged launches keep
// diagnostics with the agent's own data. Log failures never become UI errors.
func diagnosticLog(cmd *exec.Cmd) *os.File {
	env := func(name string) string {
		var value string
		for _, entry := range cmd.Environ() {
			key, v, _ := strings.Cut(entry, "=")
			if strings.EqualFold(key, name) {
				value = v
			}
		}
		return value
	}
	path := env("MINICODE_BRIDGE_LOG")
	if path == "" {
		home := env("HOME")
		if home == "" {
			home = env("USERPROFILE")
		}
		if home == "" {
			home, _ = os.UserHomeDir()
		}
		if home == "" {
			return nil
		}
		path = filepath.Join(home, ".minicode", "logs", "agent-stderr.log")
	}
	if os.MkdirAll(filepath.Dir(path), 0700) != nil {
		return nil
	}
	file, _ := os.OpenFile(path, os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0600)
	return file
}
