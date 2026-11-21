package main

import (
	"context"
	"fmt"
	"github.com/egoist/mygo/ui"
	"minicode.desktop/internal/model"
	"reflect"
	"slices"
	"sync"
	"time"
)

func (d *desktop) initialize() {
	v := d.current()
	wasFailed := d.bridgeFailed
	var result struct {
		ProtocolVersion   int             `json:"protocolVersion"`
		MinClientProtocol int             `json:"minClientProtocol"`
		Capabilities      []string        `json:"capabilities"`
		Sessions          []model.Session `json:"sessions"`
		Models            []model.Model   `json:"models"`
		Commands          []model.Command `json:"commands"`
		State             model.State     `json:"state"`
	}
	d.request(v, "initialize", nil, &result, func(err error) {
		if err != nil {
			d.error(v, err)
			return
		}
		if d.bridgeFailed && !wasFailed {
			return
		}
		d.bridgeFailed, d.bridgeError = false, ""
		version := result.ProtocolVersion
		if version == 0 {
			version = result.State.ProtocolVersion
		}
		if message, tone := protocolProblem(version, result.MinClientProtocol); message != "" {
			d.toast(tr("协议版本不匹配"), message, tone)
		}
		d.sessions, d.models, d.commands = result.Sessions, result.Models, result.Commands
		d.messageQueue = slices.Contains(result.Capabilities, "messageQueue")
		v.State = result.State
		v.Loaded = true
		if v.Workspace != "" {
			d.refresh(v, true)
		}
	})
}

func (d *desktop) refreshSessions(v *conversation) {
	d.sessionVersion++
	version := d.sessionVersion
	var result []model.Session
	d.request(v, "listSessions", nil, &result, func(err error) {
		if version != d.sessionVersion {
			return
		}
		if err != nil {
			d.error(v, err)
		} else {
			d.sessions = result
		}
	})
}

func (d *desktop) refresh(v *conversation, full bool) {
	if v.Workspace == "" || d.files == nil || d.client == nil {
		return
	}
	v.refreshVersion++
	version := v.refreshVersion
	root := v.Workspace
	params := d.params(v, nil)
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 35*time.Second)
		defer cancel()
		var changes []model.Change
		var files []string
		var tasks []model.Task
		var capabilities model.Capabilities
		var changesErr, filesErr, tasksErr, capsErr error
		var wg sync.WaitGroup
		wg.Add(2)
		go func() { defer wg.Done(); changes, changesErr = d.files.Changes(ctx, root) }()
		go func() { defer wg.Done(); tasksErr = d.client.Request(ctx, "listTasks", params, &tasks) }()
		if full {
			wg.Add(2)
			go func() { defer wg.Done(); files, filesErr = d.files.Files(ctx, root, "") }()
			go func() { defer wg.Done(); capsErr = d.client.Request(ctx, "getCapabilities", params, &capabilities) }()
		}
		wg.Wait()
		d.schedule(func() {
			if version != v.refreshVersion {
				return
			}
			if changesErr == nil {
				v.Changes = changes
			} else {
				d.error(v, changesErr)
			}
			if tasksErr == nil {
				v.Tasks = tasks
			} else {
				d.error(v, tasksErr)
			}
			if full {
				if filesErr == nil {
					v.Files = files
					v.FilesVersion++
				} else {
					d.error(v, filesErr)
				}
				if capsErr == nil {
					v.Capabilities = capabilities
				} else {
					d.error(v, capsErr)
				}
			}
		})
	}()
}

func (d *desktop) refreshServerQueue(v *conversation) {
	if !d.messageQueue || v.SessionID == "" {
		return
	}
	v.queueRevision++
	revision, session := v.queueRevision, v.SessionID
	var queued []queuedPrompt
	d.request(v, "listQueue", nil, &queued, func(err error) {
		if err == nil && revision == v.queueRevision && session == v.SessionID {
			v.Queue = queued
		}
	})
}
func (d *desktop) enqueuePrompt(v *conversation, text, mode string, attachments []model.Attachment) {
	d.request(v, "enqueuePrompt", map[string]any{"text": text, "mode": mode, "attachments": attachments}, nil, func(err error) {
		if err != nil {
			d.error(v, err)
			if v.Draft == "" {
				v.Draft = text
				v.Attachments = attachments
			}
			return
		}
		d.refreshServerQueue(v)
	})
}
func (d *desktop) deleteQueuedPrompt(v *conversation, index int) {
	if index < 0 || index >= len(v.Queue) {
		return
	}
	if !d.messageQueue {
		v.Queue = append(v.Queue[:index], v.Queue[index+1:]...)
		return
	}
	d.request(v, "deleteQueuedPrompt", map[string]any{"id": v.Queue[index].ID}, nil, func(err error) { d.error(v, err); d.refreshServerQueue(v) })
}

func (d *desktop) refreshState(v *conversation) {
	v.stateVersion++
	version := v.stateVersion
	var state model.State
	d.request(v, "getState", nil, &state, func(err error) {
		if version != v.stateVersion {
			return
		}
		if err != nil {
			d.error(v, err)
		} else {
			v.State = state
		}
	})
}

func (d *desktop) changeState(method string, params map[string]any) {
	v := d.current()
	v.stateVersion++
	version := v.stateVersion
	var state model.State
	d.request(v, method, params, &state, func(err error) {
		if version != v.stateVersion {
			return
		}
		if err != nil {
			d.error(v, err)
		} else {
			v.State = state
		}
	})
}

func (d *desktop) activate(key string) {
	if d.views[key] == nil {
		return
	}
	d.navigationVersion++
	d.active = key
	d.views[key].Unread = false
	d.preview = nil
	d.fileQuery = ""
	d.completion = nil
	d.focusPrompt = true
}

func (d *desktop) newSession(root string) {
	source := d.current()
	key := fmt.Sprintf("draft-%d", d.next.Add(1))
	for k, v := range d.views {
		if k != source.Key && v.SessionID == "" && !v.Busy && len(v.Items) == 0 && v.Draft == "" && len(v.Attachments) == 0 && !v.Uploading && v.Workspace == root {
			key = k
			break
		}
	}
	v := d.views[key]
	if v == nil {
		v = newConversation(key, root, source.State)
		d.views[key] = v
	}
	d.activate(key)
	var state model.State
	d.request(v, "resetSession", map[string]any{"sourceClientKey": source.Key}, &state, func(err error) {
		if err != nil {
			d.error(v, err)
			return
		}
		v.State = state
		v.Loaded = true
		d.rememberWorkspace(root)
		d.refresh(v, true)
	})
}

func (d *desktop) resumeSession(v *conversation, id string, navigation uint64) {
	open := func(sessions []model.Session) bool {
		for _, session := range sessions {
			if session.ID == id {
				d.openSession(session)
				return true
			}
		}
		return false
	}
	if open(d.sessions) {
		return
	}
	d.sessionVersion++
	version := d.sessionVersion
	var sessions []model.Session
	d.request(v, "listSessions", nil, &sessions, func(err error) {
		if navigation != d.navigationVersion || d.active != v.Key {
			return
		}
		if err != nil {
			d.error(v, err)
			return
		}
		if version == d.sessionVersion {
			d.sessions = sessions
		}
		if !open(sessions) {
			d.error(v, fmt.Errorf("无法找到会话：%s", id))
		}
	})
}

func (d *desktop) openSession(session model.Session) {
	source := d.active
	key := session.ID
	for k, v := range d.views {
		if v.SessionID == session.ID {
			key = k
			break
		}
	}
	v := d.views[key]
	if v == nil {
		v = newConversation(key, session.Workspace, initialState())
		v.SessionID = session.ID
		d.views[key] = v
	}
	d.activate(key)
	if v.Loaded || v.Busy {
		d.refresh(v, true)
		return
	}
	v.Loading = true
	var state model.State
	d.request(v, "selectSession", map[string]any{"sourceClientKey": source, "deferRuntime": true}, &state, func(err error) {
		if err != nil {
			v.Loading = false
			d.error(v, err)
			return
		}
		v.State = state
		v.Busy = state.Running
		var detail model.Detail
		d.request(v, "getSession", nil, &detail, func(err error) {
			v.Loading = false
			if err != nil {
				d.error(v, err)
				return
			}
			v.History = &detail
			v.Items = model.History(detail)
			v.HistoryCount = len(v.Items)
			v.rebuildRows()
			v.Trace = recentTrace(detail.Events)
			v.Loaded = true
			v.Error = ""
			d.rememberWorkspace(v.Workspace)
			d.refresh(v, true)
		})
	})
}

func (d *desktop) loadEarlierHistory(v *conversation) {
	if v.HistoryLoading || v.History == nil || !v.History.Cursor.HasMore() {
		return
	}
	history := v.History
	v.HistoryLoading = true
	var page model.Detail
	d.request(v, "getSessionPage", map[string]any{"cursor": history.Cursor}, &page, func(err error) {
		v.HistoryLoading = false
		if err != nil {
			d.error(v, err)
			return
		}
		if v.History != history {
			return
		}
		merged := *history
		merged.Messages = append(page.Messages, history.Messages...)
		merged.Events = append(page.Events, history.Events...)
		if len(page.Messages) > 0 {
			merged.MessageOffset = page.MessageOffset
		}
		merged.Cursor = page.Cursor
		items := model.History(merged)
		baseline := map[string]*model.FeedItem{}
		for _, item := range model.History(*history) {
			baseline[item.ID] = item
		}
		changed := map[string]*model.FeedItem{}
		for _, item := range v.Items[:v.HistoryCount] {
			if !reflect.DeepEqual(item, baseline[item.ID]) {
				changed[item.ID] = item
			}
		}
		for i, item := range items {
			if live := changed[item.ID]; live != nil {
				items[i] = live
			}
		}
		v.Items = append(items, v.Items[v.HistoryCount:]...)
		v.HistoryCount = len(items)
		v.History = &merged
		v.rebuildRows()
		v.List.ScrollTo(0, ui.Start)
	})
}

func (d *desktop) sessionAction(method, id string, params map[string]any) {
	v := d.current()
	if params == nil {
		params = map[string]any{}
	}
	params["sessionId"] = id
	var sessions []model.Session
	d.request(v, method, params, &sessions, func(err error) {
		if err != nil {
			d.error(v, err)
			return
		}
		d.sessions = sessions
		if method == "deleteSession" {
			activeDeleted := false
			for key, view := range d.views {
				if view.SessionID == id {
					if d.active == key {
						activeDeleted = true
					}
					delete(d.views, key)
				}
			}
			if activeDeleted || len(d.views) == 0 {
				key := fmt.Sprint("draft-", d.next.Add(1))
				d.views[key] = newConversation(key, v.Workspace, v.State)
				d.activate(key)
			}
		}
	})
}
