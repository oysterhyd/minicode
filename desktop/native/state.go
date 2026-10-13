package main

import (
	"context"
	"encoding/json"
	"fmt"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/egoist/mygo"
	"github.com/egoist/mygo/ui"
	"minicode.desktop/internal/bridge"
	"minicode.desktop/internal/model"
	"minicode.desktop/internal/workspace"
)

type requester interface {
	Request(context.Context, string, any, any) error
}

type fileService interface {
	Changes(context.Context, string) ([]model.Change, error)
	Files(context.Context, string, string) ([]string, error)
	Diff(context.Context, string, string) (string, error)
	Stage(context.Context, string, string) error
	StageAll(context.Context, string) error
	Unstage(context.Context, string, string) error
}

type queuedPrompt struct {
	Text        string
	Attachments []model.Attachment
}

type conversation struct {
	Key, SessionID, Workspace     string
	State                         model.State
	Items                         []*model.FeedItem
	Rows                          []model.FeedRow
	toolItems                     map[string]*model.FeedItem
	thinkingCount                 int
	Trace                         []model.Event
	Tasks                         []model.Task
	Changes                       []model.Change
	Files                         []string
	FilesVersion                  uint64
	Capabilities                  model.Capabilities
	Draft                         string
	Queue                         []queuedPrompt
	Attachments                   []model.Attachment
	Uploading                     bool
	Busy, Loading, Loaded, Unread bool
	CancelPending                 bool
	Error                         string
	StartedAt                     time.Time
	List                          ui.ListState
	Open                          map[string]bool
	Artifacts                     map[string]*artifact
	stream                        *model.FeedItem
	streamText                    strings.Builder
	reasoning                     *model.FeedItem
	reasoningText                 strings.Builder
	version                       uint64
	refreshVersion                uint64
	stateVersion                  uint64
	refreshPending                bool
}

type artifact struct {
	Text    string
	Total   int
	HasMore bool
	Loading bool
	Error   string
}

type desktop struct {
	views                                  map[string]*conversation
	active                                 string
	sessions                               []model.Session
	models                                 []model.Model
	commands                               []model.Command
	config                                 model.Configuration
	discovery                              model.Discovery
	client                                 requester
	files                                  fileService
	window                                 *mygo.Window
	update                                 func(func())
	notify                                 func(string, string, string)
	preferences                            preferences
	store                                  *preferenceStore
	root                                   string
	recent                                 []string
	next                                   atomic.Uint64
	eventsMu                               sync.Mutex
	events                                 []bridge.Event
	eventsScheduled                        bool
	closed                                 atomic.Bool
	sessionVersion                         uint64
	settings                               *settingsState
	modal                                  string
	paletteQuery                           string
	paletteIndex                           int
	renameID, renameText, deleteID         string
	search                                 string
	collapsedProjects                      map[string]bool
	tab                                    int
	fileQuery                              string
	folders                                map[string]bool
	preview                                *previewState
	contextOpen, modelOpen, permissionOpen bool
	completion                             *completionState
	historyIndex                           int
	historyDraft                           string
	history                                []string
	focusPrompt                            bool
	focusSearch                            bool
	toasts                                 []toast
	lastDark                               bool
	renderCache                            *renderCache
	treeCache                              *treeCache
	folderRevision                         uint64
	frameObserved                          func()
	framePainted                           func()
	frameStarted                           float64
	images                                 map[string]*messageImage
	updates                                updateService
}

type toast struct {
	ID                             uint64
	Title, Detail, Tone, SessionID string
	At                             time.Time
}
type previewState struct {
	Kind, Path, Status, Text, Error string
	Loading, Busy                   bool
	Version                         uint64
	Lines                           []codeLine
	Diff                            []modelDiffLine
	Expanded                        map[int]bool
	List                            ui.ListState
}

func newDesktop(update func(func())) *desktop {
	d := &desktop{views: map[string]*conversation{}, active: "draft-0", update: update,
		preferences: defaultPreferences(), collapsedProjects: map[string]bool{},
		folders: map[string]bool{}, historyIndex: -1, renderCache: newRenderCache()}
	d.views[d.active] = newConversation(d.active, "", initialState())
	return d
}

func initialState() model.State {
	return model.State{Effort: "off", PermissionMode: "default", ContextWindow: 200000,
		Breakdown: map[string]int{"system": 0, "tools": 0, "messages": 0}}
}

func newConversation(key, root string, state model.State) *conversation {
	state.SessionID = ""
	state.Usage = nil
	state.Statistics = nil
	state.Rounds = 0
	state.ContextTokens = 0
	state.Breakdown = map[string]int{"system": 0, "tools": 0, "messages": 0}
	state.TaskPending = false
	state.Acceptance = ""
	state.AlwaysAllow = nil
	return &conversation{Key: key, Workspace: root, State: state, Open: map[string]bool{}, Artifacts: map[string]*artifact{}}
}

func (d *desktop) current() *conversation { return d.views[d.active] }
func (d *desktop) schedule(fn func()) {
	if !d.closed.Load() {
		d.update(func() {
			if !d.closed.Load() {
				fn()
			}
		})
	}
}

func (d *desktop) params(v *conversation, extra map[string]any) map[string]any {
	params := map[string]any{"workspace": v.Workspace, "sessionId": nil, "clientKey": v.Key}
	if v.SessionID != "" {
		params["sessionId"] = v.SessionID
	}
	for key, value := range extra {
		params[key] = value
	}
	return params
}

func (d *desktop) request(v *conversation, method string, extra map[string]any, target any, done func(error)) {
	if d.client == nil {
		done(fmt.Errorf("Agent bridge 不可用"))
		return
	}
	params := d.params(v, extra)
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 35*time.Second)
		defer cancel()
		err := d.client.Request(ctx, method, params, target)
		d.schedule(func() { done(err) })
	}()
}

func (d *desktop) error(v *conversation, err error) {
	if err != nil {
		v.Error = err.Error()
	}
}

func (d *desktop) toast(title, detail, tone string) {
	d.toasts = append(d.toasts, toast{ID: d.next.Add(1), Title: title, Detail: detail, Tone: tone, At: time.Now()})
}

func (d *desktop) initialize() {
	v := d.current()
	var result struct {
		Sessions []model.Session `json:"sessions"`
		Models   []model.Model   `json:"models"`
		Commands []model.Command `json:"commands"`
		State    model.State     `json:"state"`
	}
	d.request(v, "initialize", nil, &result, func(err error) {
		if err != nil {
			d.error(v, err)
			return
		}
		d.sessions, d.models, d.commands = result.Sessions, result.Models, result.Commands
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
	d.request(v, "selectSession", map[string]any{"sourceClientKey": source}, &state, func(err error) {
		if err != nil {
			v.Loading = false
			d.error(v, err)
			return
		}
		v.State = state
		var detail model.Detail
		d.request(v, "getSession", nil, &detail, func(err error) {
			v.Loading = false
			if err != nil {
				d.error(v, err)
				return
			}
			v.Items = model.History(detail)
			v.rebuildRows()
			v.Trace = recentTrace(detail.Events)
			v.Loaded = true
			v.Error = ""
			d.rememberWorkspace(v.Workspace)
			d.refresh(v, true)
		})
	})
}

func (v *conversation) rebuildRows() {
	v.version++
	v.Rows = model.GroupFeed(v.Items)
	clear(v.toolItems)
	v.thinkingCount = 0
	for _, item := range v.Items {
		v.indexItem(item)
	}
}

func (v *conversation) indexItem(item *model.FeedItem) {
	if item.Kind == "tool" {
		if v.toolItems == nil {
			v.toolItems = map[string]*model.FeedItem{}
		}
		v.toolItems[item.ID] = item
	} else if item.Kind == "thinking" {
		v.thinkingCount++
	}
}

func (v *conversation) append(items ...*model.FeedItem) {
	position := len(v.Items)
	v.Items = append(v.Items, items...)
	for i, item := range items {
		v.Rows = model.AppendFeed(v.Rows, item, position+i)
		v.indexItem(item)
	}
	v.version++
}
func (v *conversation) removeThinking() {
	if v.thinkingCount == 0 {
		return
	}
	v.Items = slices.DeleteFunc(v.Items, func(item *model.FeedItem) bool {
		return item.Kind == "thinking"
	})
	v.rebuildRows()
}

func (d *desktop) send(text string, v *conversation) {
	d.sendAttached(text, v, nil)
}

func (d *desktop) sendAttached(text string, v *conversation, attachments []model.Attachment) {
	text = strings.TrimSpace(text)
	if text == "" && len(attachments) > 0 {
		text = "请分析这些附件"
	}
	if text == "" || v.Loading {
		return
	}
	if strings.HasPrefix(text, "/") {
		if len(attachments) > 0 {
			v.Error = "附件不能随斜杠命令发送"
			v.Draft = text
			v.Attachments = attachments
			return
		}
		d.slash(text, v)
		return
	}
	if v.Workspace == "" {
		v.Error = "请先选择本地工作区"
		v.Draft, v.Attachments = text, attachments
		return
	}
	if problem := d.attachmentProblem(v, attachments); problem != "" {
		v.Error = problem
		v.Draft = text
		v.Attachments = attachments
		return
	}
	if v.Busy {
		v.Queue = append(v.Queue, queuedPrompt{Text: text, Attachments: attachments})
		return
	}
	v.Error = ""
	v.Busy = true
	v.CancelPending = false
	v.Loaded = true
	v.StartedAt = time.Now()
	d.rememberPrompt(text)
	user := &model.FeedItem{ID: fmt.Sprint("user-", d.next.Add(1)), Kind: "user", Text: text, Attachments: attachments, StartedAt: v.StartedAt}
	v.append(user,
		&model.FeedItem{ID: fmt.Sprint("thinking-", d.next.Add(1)), Kind: "thinking", Text: "正在思考", StartedAt: v.StartedAt})
	var result struct {
		SessionID string `json:"sessionId"`
	}
	d.request(v, "sendPrompt", map[string]any{"text": text, "model": v.State.Model, "attachments": attachments}, &result, func(err error) {
		if err != nil {
			v.Busy = false
			v.StartedAt = time.Time{}
			v.removeThinking()
			v.Items = slices.DeleteFunc(v.Items, func(item *model.FeedItem) bool { return item == user })
			v.rebuildRows()
			if v.Draft == "" && len(v.Attachments) == 0 {
				v.Draft = text
				v.Attachments = attachments
			} else {
				v.Queue = append([]queuedPrompt{{Text: text, Attachments: attachments}}, v.Queue...)
			}
			d.error(v, err)
			return
		}
		v.SessionID = result.SessionID
		d.refreshSessions(v)
	})
}

func (d *desktop) submit() {
	v := d.current()
	if v.Uploading || v.Loading {
		return
	}
	if v.Busy && strings.TrimSpace(v.Draft) == "" && len(v.Attachments) == 0 {
		d.cancel(v)
		return
	}
	text := v.Draft
	attachments := slices.Clone(v.Attachments)
	if strings.TrimSpace(text) == "" && len(attachments) == 0 && len(v.Queue) > 0 && !v.Busy {
		text = v.Queue[0].Text
		attachments = v.Queue[0].Attachments
		v.Queue = v.Queue[1:]
	}
	if strings.TrimSpace(text) == "" && len(attachments) == 0 {
		return
	}
	v.Draft = ""
	v.Attachments = nil
	d.completion = nil
	d.historyIndex = -1
	d.sendAttached(text, v, attachments)
}

func (d *desktop) cancel(v *conversation) {
	if !v.Busy || v.CancelPending {
		return
	}
	v.CancelPending = true
	d.request(v, "cancelTurn", nil, nil, func(err error) {
		if err != nil {
			v.CancelPending = false
			d.error(v, err)
		}
	})
}

func (d *desktop) decide(item *model.FeedItem, granted, remember bool) {
	v := d.current()
	id := item.ApprovalID
	if id == "" || item.Deciding {
		return
	}
	item.Deciding = true
	params := map[string]any{"approvalId": id, "granted": granted}
	if granted && remember {
		params["remember"] = "session"
	}
	var handled bool
	d.request(v, "resolveApproval", params, &handled, func(err error) {
		item.Deciding = false
		if err != nil {
			d.error(v, err)
			return
		}
		if !handled {
			d.refreshState(v)
			return
		}
		item.ApprovalID = ""
		item.Granted = granted
		item.Version++
		d.refreshState(v)
	})
}

func (d *desktop) slash(text string, v *conversation) {
	command := strings.Fields(text)
	if len(command) == 0 {
		return
	}
	switch command[0] {
	case "/clear":
		v.Items = nil
		v.rebuildRows()
		return
	case "/new":
		d.newSession(v.Workspace)
		return
	case "/sessions":
		d.modal = "palette"
		d.paletteQuery = ""
		return
	case "/resume":
		if len(command) == 1 {
			d.modal = "palette"
			d.paletteQuery = ""
			return
		}
	case "/model":
		if len(command) == 1 {
			d.modelOpen = true
			return
		}
	case "/effort":
		if len(command) == 1 {
			d.modelOpen = true
			return
		}
	case "/permissions":
		if len(command) == 1 {
			d.permissionOpen = true
			return
		}
	case "/help":
		var text strings.Builder
		for _, cmd := range d.commands {
			fmt.Fprintf(&text, "%s  %s\n", cmd.Usage, cmd.Summary)
		}
		v.append(&model.FeedItem{ID: fmt.Sprint("help-", d.next.Add(1)), Kind: "notice", Text: text.String()})
		return
	}
	var result struct {
		Action    string       `json:"action"`
		Message   string       `json:"message"`
		SessionID string       `json:"sessionId"`
		State     *model.State `json:"state"`
	}
	continuing := command[0] == "/continue" && !v.Busy
	if continuing {
		v.Busy = true
		v.CancelPending = false
		v.StartedAt = time.Now()
	}
	d.request(v, "runSlash", map[string]any{"text": text}, &result, func(err error) {
		if err != nil {
			if continuing {
				v.Busy = false
				v.StartedAt = time.Time{}
			}
			d.error(v, err)
			return
		}
		if result.State != nil {
			v.State = *result.State
		}
		if result.Message != "" {
			v.append(&model.FeedItem{ID: fmt.Sprint("slash-", d.next.Add(1)), Kind: "notice", Text: result.Message})
		}
		switch result.Action {
		case "new":
			d.newSession(v.Workspace)
		case "clear":
			v.Items = nil
			v.rebuildRows()
		case "continue":
			if result.SessionID != "" {
				v.SessionID = result.SessionID
			}
		case "skills":
			d.openSettings("skills")
		case "exit":
			if d.window != nil {
				mygo.App.Quit()
			}
		case "resume":
			for _, session := range d.sessions {
				if session.ID == result.SessionID {
					d.openSession(session)
					break
				}
			}
		}
	})
}

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
	if event.Event == "bridge_error" {
		if !event.Fatal && !strings.Contains(event.Error, "Error") && !strings.Contains(event.Error, "exited") && !strings.Contains(event.Error, "invalid") && !strings.Contains(event.Error, "Traceback") {
			return
		}
		for _, v := range d.views {
			v.Error = event.Error
			if event.Fatal || strings.Contains(event.Error, "exited") || strings.Contains(event.Error, "invalid") {
				d.finish(v, bridge.Event{Event: "run_error", Error: event.Error, Fatal: true})
			}
		}
		return
	}
	v := d.views[event.ClientKey]
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
	switch event.Event {
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
			for i := len(v.Items) - 1; i >= 0; i-- {
				item := v.Items[i]
				if item.Kind == "tool" && item.Pending && item.Name == request.ToolName {
					item.Auto = true
					break
				}
			}
			return
		}
		v.finishStream()
		v.removeThinking()
		v.append(&model.FeedItem{ID: "approval-" + event.ApprovalID, Kind: "approval", Name: request.ToolName, Args: request.Arguments, ApprovalID: event.ApprovalID})
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
		v.append(&model.FeedItem{ID: fmt.Sprintf("retry-%d", event.Seq), Kind: "notice", Tone: "warning", Text: fmt.Sprintf("模型请求暂时失败，%g 秒后重试（第 %g 次）。", model.Number(data["delay_s"]), model.Number(data["next_attempt"]))})
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
			item.EndedAt = stamp
			item.Output = model.String(data["output_detail"])
			if item.Output == "" {
				item.Output = model.String(data["output_preview"])
			}
			if err := model.String(data["error"]); err != "" {
				if item.Output != "" {
					item.Output += "\n"
				}
				item.Output += err
			}
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
	case "approval_decision":
		for _, item := range v.Items {
			if item.Kind == "approval" && item.ApprovalID != "" {
				item.ApprovalID = ""
				item.Granted = model.Bool(data["granted"])
				item.Version++
			}
		}
	case "subagent_start":
		for _, item := range v.Items {
			if item.Kind == "tool" && item.Name == "delegate" && model.String(item.Args["kind"]) == model.String(data["kind"]) && model.String(item.Args["task"]) == model.String(data["task"]) {
				item.ChildSessionID = model.String(data["child_session_id"])
			}
		}
	case "subagent_result", "context_compacted":
		d.refreshState(v)
	}
	notice, tone := "", "info"
	switch event.Type {
	case "context_compacted":
		notice = "上下文已自动压缩，较早的内容已归档"
	case "goal_check":
		notice = "验收检查：仍需改进"
		if model.Bool(data["passed"]) {
			notice = "验收检查：通过"
		}
	case "background_job_completed":
		notice = "后台命令 " + model.String(data["job_id"]) + "：已完成"
	case "background_job_lost":
		notice = "后台命令 " + model.String(data["job_id"]) + "：结果丢失"
		tone = "warning"
	case "side_effect_unknown":
		notice = "工具执行结果需复核：" + model.String(data["name"])
		tone = "warning"
	}
	if notice != "" {
		v.append(&model.FeedItem{ID: fmt.Sprintf("notice-%d", event.Seq), Kind: "notice", Text: notice, Tone: tone})
	}
}

func (d *desktop) finish(v *conversation, event bridge.Event) {
	v.stateVersion++
	cancelled := v.CancelPending
	v.CancelPending = false
	var result model.RunResult
	_ = json.Unmarshal(event.Result, &result)
	if v.reasoning != nil && v.reasoning.Pending {
		v.reasoning.Interrupted = event.Event == "run_error" || result.ExitReason == "cancelled" || result.ExitReason == "provider_error"
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
	d.signal(v, "任务已结束", v.title(d.sessions))
	if ok && !cancelled && len(v.Queue) > 0 {
		next := v.Queue[0]
		v.Queue = v.Queue[1:]
		d.sendAttached(next.Text, v, next.Attachments)
	}
}

func (v *conversation) title(sessions []model.Session) string {
	for _, session := range sessions {
		if session.ID == v.SessionID {
			return session.Title
		}
	}
	for _, item := range v.Items {
		if item.Kind == "user" {
			title := strings.Split(item.Text, "\n")[0]
			runes := []rune(title)
			if len(runes) > 72 {
				title = string(runes[:72])
			}
			return title
		}
	}
	return tr("新建任务")
}

func (d *desktop) signal(v *conversation, title, body string) {
	if d.notify != nil && d.preferences.Notifications {
		d.notify(tr(title), body, v.SessionID)
	}
	if d.active != v.Key {
		d.toasts = append(d.toasts, toast{ID: d.next.Add(1), Title: title, Detail: body, Tone: "info", SessionID: v.SessionID, At: time.Now()})
	}
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

func (d *desktop) loadConfiguration() {
	v := d.current()
	var config model.Configuration
	d.request(v, "getConfiguration", nil, &config, func(err error) {
		if err != nil {
			d.error(v, err)
		} else {
			d.config = config
		}
	})
}

func (d *desktop) refreshConfiguration() {
	v := d.current()
	var models []model.Model
	d.request(v, "listModels", nil, &models, func(err error) {
		if err != nil {
			d.error(v, err)
		} else {
			d.models = models
			d.refreshState(v)
			d.refresh(v, true)
		}
	})
	d.loadConfiguration()
}

func (d *desktop) loadArtifact(item *model.FeedItem) {
	v := d.current()
	a := v.Artifacts[item.ID]
	if a == nil {
		a = &artifact{}
		v.Artifacts[item.ID] = a
	}
	if a.Loading {
		return
	}
	a.Loading = true
	var result struct {
		Text    string `json:"text"`
		Total   int    `json:"total"`
		HasMore bool   `json:"hasMore"`
	}
	offset := len([]rune(a.Text))
	d.request(v, "readArtifact", map[string]any{"artifactId": item.ArtifactID, "offset": offset}, &result, func(err error) {
		a.Loading = false
		if err != nil {
			a.Error = err.Error()
			return
		}
		a.Text += result.Text
		a.Total = result.Total
		a.HasMore = result.HasMore
	})
}

func (d *desktop) openPreview(kind, path, status string) {
	p := &previewState{Kind: kind, Path: path, Status: status, Loading: true, Expanded: map[int]bool{}}
	d.preview = p
	root := d.current().Workspace
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
		defer cancel()
		var text string
		var err error
		if kind == "diff" {
			text, err = d.files.Diff(ctx, root, path)
		} else {
			var truncated bool
			text, truncated, err = workspace.Read(root, path)
			if truncated {
				text += "\n...[文件预览已截断，使用 Agent 的 read 工具分页查看]"
			}
		}
		d.schedule(func() {
			if d.preview != p {
				return
			}
			p.Loading = false
			if err != nil {
				p.Error = err.Error()
				return
			}
			p.Text = text
			p.Version++
			if kind == "diff" {
				p.Diff = convertDiff(text)
			} else {
				p.Lines = highlight(text, filepath.Ext(path))
			}
		})
	}()
}

func (d *desktop) stage(path string, all, unstage bool) {
	v := d.current()
	root := v.Workspace
	p := d.preview
	if p != nil {
		p.Busy = true
	}
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
		defer cancel()
		var err error
		if all {
			err = d.files.StageAll(ctx, root)
		} else if unstage {
			err = d.files.Unstage(ctx, root, path)
		} else {
			err = d.files.Stage(ctx, root, path)
		}
		d.schedule(func() {
			if p != nil {
				p.Busy = false
			}
			if err != nil {
				d.error(v, err)
				return
			}
			title := "已暂存"
			if unstage {
				title = "已取消暂存"
			}
			d.toast(title, path, "success")
			d.refresh(v, false)
			if p != nil {
				if unstage {
					p.Status = " M"
				} else {
					p.Status = "M "
				}
			}
		})
	}()
}
