package bridge

import (
	"context"
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"sync"
	"testing"
	"time"
)

func realClient(t *testing.T, script string) (*Client, chan Event, string) {
	t.Helper()
	root, err := filepath.Abs("../../../..")
	if err != nil {
		t.Fatal(err)
	}
	python := filepath.Join(root, ".venv", "Scripts", "python.exe")
	if _, err := os.Stat(python); err != nil {
		python = os.Getenv("MINICODE_PYTHON")
		if python == "" {
			python = "python"
		}
		python, err = exec.LookPath(python)
		if err != nil {
			t.Skip("Python interpreter not installed")
		}
	}
	profile := t.TempDir()
	cmd := Launch(root, "", profile)
	cmd.Path, cmd.Args[0] = python, python
	cmd.Env = append(without(cmd.Env, "HOME", "USERPROFILE", "MINICODE_DB_PATH", "MINICODE_FAKE_SCRIPT", "ANTHROPIC_API_KEY", "COMMANDCODE_API_KEY"), "HOME="+profile, "USERPROFILE="+profile, "MINICODE_DB_PATH="+filepath.Join(profile, "sessions.db"))
	if script != "" {
		file := filepath.Join(profile, "script.json")
		if err := os.WriteFile(file, []byte(script), 0600); err != nil {
			t.Fatal(err)
		}
		cmd.Env = append(cmd.Env, "MINICODE_FAKE_SCRIPT="+file)
	}
	events := make(chan Event, 512)
	c, err := Start(cmd, func(e Event) { events <- e })
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = c.Close() })
	return c, events, profile
}

func request(t *testing.T, c *Client, method string, params map[string]any, target any) {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	if err := c.Request(ctx, method, params, target); err != nil {
		t.Fatalf("%s: %v", method, err)
	}
}

func TestRealBridgeConcurrentRequestsAndConfiguration(t *testing.T) {
	c, _, _ := realClient(t, "")
	var wg sync.WaitGroup
	failures := make(chan error, 12)
	for i := 0; i < 12; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
			defer cancel()
			var state struct {
				Version int `json:"protocolVersion"`
			}
			err := c.Request(ctx, "getState", map[string]any{"clientKey": "native-test"}, &state)
			if err == nil && state.Version != 2 {
				t.Error("wrong bridge protocol")
			}
			failures <- err
		}()
	}
	wg.Wait()
	close(failures)
	for err := range failures {
		if err != nil {
			t.Fatal(err)
		}
	}
	agent := map[string]any{"name": "native-review", "label": "Native reviewer", "description": "Review changes", "instructions": "Read files and report evidence.", "tools": []string{"read", "grep"}, "inheritTools": false}
	var agents []map[string]any
	request(t, c, "saveAgent", map[string]any{"agent": agent}, &agents)
	agent["originalName"] = "native-review"
	agent["name"] = "native-review-edited"
	agent["instructions"] = "Read files and verify tests."
	request(t, c, "saveAgent", map[string]any{"agent": agent}, &agents)
	found := false
	for _, a := range agents {
		if a["name"] == "native-review-edited" {
			found = true
		}
		if a["name"] == "native-review" {
			t.Fatal("renamed delegate left duplicate")
		}
	}
	if !found {
		t.Fatal("edited delegate absent")
	}
	request(t, c, "deleteAgent", map[string]any{"name": "native-review-edited"}, &agents)
	var config struct {
		Services []struct {
			ID, Name  string
			APIKey    string `json:"apiKey"`
			HasAPIKey bool   `json:"hasApiKey"`
		} `json:"services"`
	}
	service := map[string]any{"name": "Native test service", "baseUrl": "https://example.test/v1", "apiStyle": "openai", "apiKey": "native-integration-placeholder", "enabled": true, "models": []map[string]any{{"modelId": "test-model", "name": "Test model", "contextWindow": 200000, "maxOutputTokens": 8192, "supportsEffort": true}}}
	request(t, c, "saveService", map[string]any{"service": service}, &config)
	id := ""
	for _, s := range config.Services {
		if s.Name == "Native test service" {
			id = s.ID
			if s.APIKey != "" || !s.HasAPIKey {
				t.Fatal("service response did not redact the credential")
			}
		}
	}
	if id == "" {
		t.Fatal("service not created")
	}
	service["id"] = id
	service["apiKey"] = ""
	service["name"] = "Edited test service"
	request(t, c, "saveService", map[string]any{"service": service}, &config)
	for _, s := range config.Services {
		if s.ID == id && !s.HasAPIKey {
			t.Fatal("blank edit erased existing credential")
		}
	}
	request(t, c, "deleteService", map[string]any{"id": id}, &config)
}

func TestRealBridgeApprovalStreamingAndSessionRecovery(t *testing.T) {
	script := `{"turns":[{"text":"Inspecting the native workspace.","tool_calls":[{"name":"write","arguments":{"path":"verified.txt","content":"Hello native\n"}}]},{"text":"Native task verified."}]}`
	c, events, profile := realClient(t, script)
	work := filepath.Join(profile, "workspace")
	if err := os.MkdirAll(work, 0700); err != nil {
		t.Fatal(err)
	}
	params := map[string]any{"clientKey": "native-work", "workspace": work}
	request(t, c, "setTuiProvider", map[string]any{"clientKey": "native-work", "provider": "fake"}, nil)
	var started struct {
		SessionID string `json:"sessionId"`
	}
	send := map[string]any{"clientKey": "native-work", "workspace": work, "text": "Write verified.txt and finish", "model": "fake"}
	request(t, c, "sendPrompt", send, &started)
	deadline := time.NewTimer(10 * time.Second)
	defer deadline.Stop()
	approved, streamed, done := false, false, false
	for !done {
		select {
		case event := <-events:
			if event.ClientKey != "" && event.ClientKey != "native-work" {
				t.Fatal("event routed to another client")
			}
			switch event.Event {
			case "text_delta":
				streamed = true
			case "approval":
				if _, err := os.Stat(filepath.Join(work, "verified.txt")); !os.IsNotExist(err) {
					t.Fatal("write executed before approval")
				}
				var handled bool
				request(t, c, "resolveApproval", map[string]any{"clientKey": "native-work", "sessionId": started.SessionID, "approvalId": event.ApprovalID, "granted": true, "remember": "session"}, &handled)
				if !handled {
					t.Fatal("approval was not accepted")
				}
				approved = true
			case "run_done":
				var result struct {
					Reason string `json:"exit_reason"`
				}
				_ = json.Unmarshal(event.Result, &result)
				if result.Reason != "completed" {
					t.Fatalf("task exit %s", result.Reason)
				}
				done = true
			case "run_error":
				t.Fatal(event.Error)
			}
		case <-deadline.C:
			t.Fatal("task did not finish")
		}
	}
	if !approved || !streamed {
		t.Fatal("approval or streaming missing")
	}
	data, err := os.ReadFile(filepath.Join(work, "verified.txt"))
	if err != nil || string(data) != "Hello native\n" {
		t.Fatal("approved write did not produce the file")
	}
	params["sessionId"] = started.SessionID
	var state struct {
		Always  []string `json:"alwaysAllow"`
		Running bool     `json:"running"`
	}
	request(t, c, "getState", params, &state)
	if len(state.Always) != 1 || state.Always[0] != "write" || state.Running {
		t.Fatal("session permission/running state not preserved")
	}
	var detail struct {
		Messages []json.RawMessage `json:"messages"`
		Events   []json.RawMessage `json:"events"`
	}
	request(t, c, "getSession", params, &detail)
	if len(detail.Messages) < 3 || len(detail.Events) == 0 {
		t.Fatal("durable history absent")
	}
	request(t, c, "renameSession", map[string]any{"sessionId": started.SessionID, "title": "Native verified task"}, nil)
	request(t, c, "pinSession", map[string]any{"sessionId": started.SessionID, "pinned": true}, nil)
	request(t, c, "getSession", map[string]any{"clientKey": "recovered", "sessionId": started.SessionID}, &detail)
	if len(detail.Messages) < 3 {
		t.Fatal("history did not recover in another client")
	}
	request(t, c, "deleteSession", map[string]any{"sessionId": started.SessionID}, nil)
}
