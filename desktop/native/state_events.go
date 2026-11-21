package main

import (
	"encoding/json"
	"errors"
	"fmt"
	"minicode.desktop/internal/bridge"
	"minicode.desktop/internal/model"
	"slices"
	"strings"
	"time"
)

func (d *desktop) queueEvent(event bridge.Event) {
	if d.closed.Load() {
		return
	}
	d.eventsMu.Lock()
	d.events = append(d.events, event)
	start := !d.eventsScheduled
	d.eventsScheduled = true
	d.eventsMu.Unlock()
	if start {
		if event.Event == "text_delta" || event.Event == "reasoning_delta" {
			time.AfterFunc(8*time.Millisecond, func() { d.schedule(d.flushEvents) })
		} else {
			d.schedule(d.flushEvents)
		}
	}
}

func (d *desktop) flushEvents() {
	d.eventsMu.Lock()
	events := d.events
	d.events = nil
	d.eventsScheduled = false
	d.eventsMu.Unlock()
	for _, event := range events {
		d.handleEvent(event)
	}
}

func (d *desktop) handleEvent(event bridge.Event) {
	if d.bridgeFailed {
		return
	}
	if event.Event == "bridge_error" && event.ClientKey == "" && event.SessionID == "" {
		if event.Error == "" {
			event.Error = "Agent bridge error"
		}
		if event.Error != d.bridgeError || event.Fatal {
			d.toast("Agent bridge", event.Error, "error")
		}
		d.bridgeError = event.Error
		if event.Fatal {
			d.bridgeFailed = true
			if d.notify != nil && d.preferences.Notifications {
				d.notify("Agent bridge", event.Error, "")
			}
			for _, v := range d.views {
				if v.Busy || v.AwaitingRun || v.State.Running || (v.stream != nil && v.stream.Pending) || (v.reasoning != nil && v.reasoning.Pending) {
					v.sendGeneration++
					d.finish(v, bridge.Event{Event: "bridge_error", Fatal: true})
				}
			}
		}
		return
	}
	v := d.views[event.ClientKey]
	if event.Event == "warning" && event.ClientKey == "" && event.SessionID == "" {
		v = d.current()
	}
	if event.SessionID != "" && (v == nil || v.SessionID == "") {
		for _, candidate := range d.views {
			if candidate.SessionID == event.SessionID {
				v = candidate
				break
			}
		}
	}
	if v == nil || (event.SessionID != "" && v.SessionID != "" && v.SessionID != event.SessionID) {
		v = nil
		for _, candidate := range d.views {
			if event.SessionID != "" && candidate.SessionID == event.SessionID {
				v = candidate
				break
			}
		}
	}
	if v == nil {
		return
	}
	if event.SessionID != "" && v.SessionID == "" {
		v.SessionID = event.SessionID
	}
	if !model.AcceptsRunEvent(v.SessionID, nil, v.State.RunID, v.AwaitingRun, v.Busy, event.SessionID, event.RunID, event.Event) {
		return
	}
	switch event.Event {
	case "usage_delta":
		if len(event.Usage) > 0 {
			var usage model.Usage
			if json.Unmarshal(event.Usage, &usage) == nil {
				v.State.Usage = &usage
			}
		}
	case "queue_changed":
		d.refreshServerQueue(v)
	case "bridge_error":
		if event.Fatal {
			event.Event = "run_error"
			d.finish(v, event)
		} else {
			d.error(v, errors.New(event.Error))
		}
	case "run_started":
		v.State.RunID = event.RunID
		v.AwaitingRun = false
		v.Busy = true
	case "warning":
		v.append(&model.FeedItem{ID: fmt.Sprint("warning-", d.next.Add(1)), Kind: "notice", Tone: "warning", Text: event.Text})
	case "reasoning_delta":
		if event.Text == "" {
			return
		}
		if v.reasoning == nil {
			v.removeThinking()
			v.reasoning = &model.FeedItem{ID: fmt.Sprint("reasoning-", d.next.Add(1)), Kind: "reasoning", Pending: true, StartedAt: time.Now()}
			v.reasoningText.Reset()
			v.append(v.reasoning)
		}
		v.reasoningText.WriteString(event.Text)
		v.reasoning.Text = v.reasoningText.String()
		v.reasoning.Pending = true
		v.reasoning.Version++
	case "text_delta":
		if v.reasoning != nil && v.reasoning.Pending {
			v.reasoning.Pending = false
			v.reasoning.EndedAt = time.Now()
		}
		if v.stream == nil {
			v.removeThinking()
			v.stream = &model.FeedItem{ID: fmt.Sprint("stream-", d.next.Add(1)), Kind: "assistant", Pending: true}
			v.streamText.Reset()
			v.append(v.stream)
		}
		v.streamText.WriteString(event.Text)
		v.stream.Text = v.streamText.String()
		v.stream.Version++
	case "approval", "approval_auto":
		var request model.ApprovalRequest
		if json.Unmarshal(event.Request, &request) != nil {
			return
		}
		if event.Event == "approval_auto" {
			callID := request.CallID
			if callID == "" {
				callID = v.approvalCallID
			}
			for i := len(v.Items) - 1; i >= 0; i-- {
				item := v.Items[i]
				if item.Kind != "tool" || !item.Pending {
					continue
				}
				if callID != "" {
					if item.ID == callID || item.CallID == callID {
						item.Auto = true
						break
					}
					continue
				}
				if item.Name == request.ToolName {
					item.Auto = true
					break
				}
			}
			return
		}
		v.finishStream()
		v.removeThinking()
		callID := request.CallID
		if callID == "" {
			callID = v.approvalCallID
		}
		if callID == v.approvalCallID {
			v.approvalCallID = ""
		}
		v.append(&model.FeedItem{ID: "approval-" + event.ApprovalID, Kind: "approval", Name: request.ToolName, Args: request.Arguments, ApprovalID: event.ApprovalID, GrantRule: request.GrantRule, CallID: callID})
		d.signal(v, "需要审批", request.ToolName)
	case "run_done", "run_error":
		d.finish(v, event)
	case "agent_event":
		var item model.Event
		if json.Unmarshal(event.Item, &item) != nil {
			return
		}
		d.runtimeEvent(v, item)
	}
}

func (v *conversation) finishStream() {
	if v.reasoning != nil {
		v.reasoning.Pending = false
		if v.reasoning.EndedAt.IsZero() {
			v.reasoning.EndedAt = time.Now()
		}
		v.reasoning.Version++
		v.reasoning = nil
	}
	v.reasoningText.Reset()
	if v.stream != nil {
		v.stream.Pending = false
		v.stream.Version++
		v.stream = nil
	}
	v.streamText.Reset()
}

func recentTrace(events []model.Event) []model.Event {
	// A suffix slice pins the complete imported trace and all its data maps.
	return slices.Clone(events[max(0, len(events)-300):])
}

func (d *desktop) runtimeEvent(v *conversation, event model.Event) {
	if event.Type == "queue_consumed" {
		if text := model.String(event.Data["text"]); text != "" {
			v.append(&model.FeedItem{ID: "queued-" + model.String(event.Data["id"]), Kind: "user", Text: text})
		}
		d.refreshServerQueue(v)
	}
	if event.Type == "tool_call_result" && strings.HasPrefix(model.String(event.Data["name"]), "task_") {
		var tasks []model.Task
		d.request(v, "listTasks", nil, &tasks, func(err error) {
			if err == nil {
				v.Tasks = tasks
			}
		})
	}
	v.Trace = append(v.Trace, event)
	if len(v.Trace) > 300 {
		v.Trace[0] = model.Event{} // Release maps in the discarded backing slot.
		v.Trace = v.Trace[len(v.Trace)-300:]
	}
	data := event.Data
	stamp := model.Time(event.Timestamp)
	switch event.Type {
	case "round_start":
		v.finishStream()
		v.removeThinking()
		v.State.Rounds = int(model.Number(data["round"]))
		v.append(&model.FeedItem{ID: fmt.Sprintf("thinking-%d", event.Seq), Kind: "thinking", Text: "正在思考", StartedAt: stamp})
	case "provider_retry":
		v.removeThinking()
		if model.Bool(data["reset_output"]) {
			v.Items = slices.DeleteFunc(v.Items, func(item *model.FeedItem) bool {
				return item == v.stream || item == v.reasoning
			})
			v.finishStream()
			v.rebuildRows()
		}
		if notice, tone := model.NoticeForAgentEvent(event.Type, data); notice != "" {
			v.append(&model.FeedItem{ID: fmt.Sprintf("retry-%d", event.Seq), Kind: "notice", Tone: tone, Text: notice})
		}
	case "assistant_message":
		stream := v.stream
		reasoning := v.reasoning
		v.finishStream()
		v.removeThinking()
		if text := model.String(data["reasoning"]); text != "" {
			if reasoning != nil {
				reasoning.Text = text
				reasoning.Version++
			} else {
				v.append(&model.FeedItem{ID: fmt.Sprintf("reasoning-%d", event.Seq), Kind: "reasoning", Text: text, EndedAt: stamp})
			}
		}
		text := model.String(data["text"])
		if stream != nil {
			if text != "" {
				stream.Text = text
				stream.ID = fmt.Sprintf("assistant-%d", event.Seq)
				stream.Version++
				if last := &v.Rows[len(v.Rows)-1]; last.Item == stream {
					last.ID = stream.ID
					v.version++
				} else {
					v.rebuildRows()
				}
			}
		} else if text != "" {
			v.append(&model.FeedItem{ID: fmt.Sprintf("assistant-%d", event.Seq), Kind: "assistant", Text: text})
		}
		d.refreshState(v)
	case "tool_call_start":
		v.finishStream()
		v.removeThinking()
		args, _ := data["arguments"].(map[string]any)
		v.append(&model.FeedItem{ID: model.String(data["call_id"]), Kind: "tool", Name: model.String(data["name"]), Args: args, Pending: true, StartedAt: stamp})
	case "tool_call_result":
		if item := v.toolItems[model.String(data["call_id"])]; item != nil {
			item.Pending = false
			item.Success = model.Bool(data["success"])
			item.Interrupted = false
			item.EndedAt = stamp
			item.Output = model.FormatToolOutput(data)
			item.ArtifactID = model.String(data["artifact_id"])
			if item.Success {
				item.Attachments = model.ImageReferences(data["images"])
			}
			item.Version++
		}
		if !v.refreshPending {
			v.refreshPending = true
			time.AfterFunc(80*time.Millisecond, func() { d.schedule(func() { v.refreshPending = false; d.refresh(v, false) }) })
		}
	case "tool_output":
		if item := v.toolItems[model.String(data["call_id"])]; item != nil {
			item.Output = model.String(data["output_preview"])
			item.Version++
		}
	case "approval_request":
		v.approvalCallID = model.String(data["call_id"])
	case "approval_decision":
		for _, item := range v.Items {
			if item.Kind == "approval" && item.ApprovalID != "" && item.CallID == model.String(data["call_id"]) {
				item.ApprovalID = ""
				item.Granted = model.Bool(data["granted"])
				item.Version++
			}
		}
	case "subagent_start", "subagent_progress", "subagent_result":
		model.ApplyChildEvent(v.Items, event)
		if event.Type == "subagent_result" {
			d.refreshState(v)
		}
	case "context_compacted":
		d.refreshState(v)
	}
	if event.Type != "provider_retry" {
		if notice, tone := model.NoticeForAgentEvent(event.Type, data); notice != "" {
			v.append(&model.FeedItem{ID: fmt.Sprintf("notice-%d", event.Seq), Kind: "notice", Text: notice, Tone: tone})
		}
	}
}

func (d *desktop) finish(v *conversation, event bridge.Event) {
	v.AwaitingRun = false
	v.stateVersion++
	cancelled := v.CancelPending
	v.CancelPending = false
	var result model.RunResult
	_ = json.Unmarshal(event.Result, &result)
	if v.reasoning != nil && v.reasoning.Pending {
		v.reasoning.Interrupted = event.Event == "run_error" || event.Event == "bridge_error" || result.ExitReason == "cancelled" || result.ExitReason == "provider_error"
	}
	v.finishStream()
	v.removeThinking()
	v.Busy = false
	v.Unread = d.active != v.Key
	if event.Event == "run_error" {
		v.Error = event.Error
	}
	lastUser := 0
	for i, item := range v.Items {
		if item.Kind == "user" {
			lastUser = i
		}
		if item.ApprovalID != "" {
			item.ApprovalID = ""
			item.Granted = false
		}
		if item.Pending {
			item.Pending = false
			item.Interrupted = item.Kind == "tool"
			item.EndedAt = time.Now()
		}
	}
	tools := 0
	for _, item := range v.Items[lastUser:] {
		if item.Kind == "tool" {
			tools++
		}
	}
	ok := event.Event == "run_done" && (result.ExitReason == "" || result.ExitReason == "completed")
	v.State.Running = false
	v.State.TaskPending = !ok
	v.State.Phase = "paused"
	if ok {
		v.State.Phase = "idle"
	}
	if !v.StartedAt.IsZero() {
		v.append(&model.FeedItem{ID: fmt.Sprint("summary-", d.next.Add(1)), Kind: "summary", Text: fmt.Sprint(tools), StartedAt: v.StartedAt, EndedAt: time.Now(), Success: ok, Interrupted: result.ExitReason == "cancelled"})
	}
	v.StartedAt = time.Time{}
	if result.ExitReason != "" && result.ExitReason != "completed" && result.ExitReason != "cancelled" {
		reason := result.Error
		if reason == "" {
			reason = result.ExitReason
		}
		v.append(&model.FeedItem{ID: fmt.Sprint("pause-", d.next.Add(1)), Kind: "notice", Tone: "warning", Text: "任务已暂停：" + reason + "。可输入 /continue 继续"})
	}
	if !event.Fatal {
		d.refreshState(v)
		d.refresh(v, true)
		d.refreshSessions(v)
	}
	if event.Event != "bridge_error" {
		d.signal(v, "任务已结束", v.title(d.sessions))
	}
	if d.messageQueue {
		d.refreshServerQueue(v)
	}
	if !d.messageQueue && ok && !cancelled && len(v.Queue) > 0 {
		next := v.Queue[0]
		v.Queue = v.Queue[1:]
		d.sendAttached(next.Text, v, next.Attachments)
	}
}

func (d *desktop) signal(v *conversation, title, body string) {
	if d.notify != nil && d.preferences.Notifications {
		d.notify(tr(title), body, v.SessionID)
	}
	if d.active != v.Key {
		d.toasts = append(d.toasts, toast{ID: d.next.Add(1), Title: title, Detail: body, Tone: "info", SessionID: v.SessionID, At: time.Now()})
	}
}
