package main

import (
	"context"
	"errors"
	"fmt"
	"github.com/egoist/mygo"
	"github.com/egoist/mygo/ui"
	"minicode.desktop/internal/bridge"
	"minicode.desktop/internal/model"
	"minicode.desktop/internal/workspace"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"sync/atomic"
	"time"
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
	ID          string `json:"id"`
	Mode        string `json:"mode"`
	Text        string
	Attachments []model.Attachment
}

type conversation struct {
	approvalCallID                string
	sendGeneration                uint64
	queueRevision                 uint64
	AwaitingRun                   bool
	History                       *model.Detail
	HistoryCount                  int
	HistoryLoading                bool
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
	messageQueue                           bool
	requestActive                          int
	requestPending                         []func()
	requestControlActive                   int
	requestControlPending                  []func()
	bridgeFailed                           bool
	bridgeError                            string
	navigationVersion                      uint64
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
	pendingCaret                           int
	caretPending                           bool
	dockHeight                             float32
	sessionListBounds                      ui.Rect
	activeRowY                             float32
	activeRowSeen                          bool
	completionUndo                         *completionUndo
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
	deadline := time.Now().Add(35 * time.Second)
	active, pending, limit := &d.requestActive, &d.requestPending, 16
	if method == "cancelTurn" || method == "resolveApproval" {
		active, pending, limit = &d.requestControlActive, &d.requestControlPending, 2
	}
	start := func() {
		*active += 1
		go func() {
			ctx, cancel := context.WithDeadline(context.Background(), deadline)
			defer cancel()
			err := ctx.Err()
			if err == nil {
				err = d.client.Request(ctx, method, params, target)
			}
			d.schedule(func() {
				*active -= 1
				if len(*pending) > 0 {
					next := (*pending)[0]
					*pending = (*pending)[1:]
					next()
				}
				done(err)
			})
		}()
	}
	if *active >= limit {
		if len(*pending) >= 128 {
			done(fmt.Errorf("Bridge request queue is full"))
			return
		}
		*pending = append(*pending, start)
	} else {
		start()
	}
}

func (d *desktop) error(v *conversation, err error) {
	if err != nil && !d.bridgeFailed {
		v.Error = err.Error()
	}
}

func (d *desktop) toast(title, detail, tone string) {
	d.toasts = append(d.toasts, toast{ID: d.next.Add(1), Title: title, Detail: detail, Tone: tone, At: time.Now()})
}

// Bridge protocol versions this desktop understands.
const protocolMin, protocolMax = 2, 2

// protocolProblem compares the agent's handshake with the supported range and
// returns an upgrade hint (and toast tone), or "" when they are compatible.
// A missing version is an older agent that only reports it in its state.
func protocolProblem(version, minClient int) (string, string) {
	switch {
	case version == 0:
		return "", ""
	case version < protocolMin:
		return trf("Agent 协议版本 %d 低于桌面端需要的 %d，请升级 MiniCode agent。", version, protocolMin), "error"
	case minClient > protocolMax:
		return trf("Agent 需要协议版本 %d 以上的客户端，请升级桌面端。", minClient), "error"
	case version > protocolMax:
		return trf("Agent 协议版本 %d 比桌面端支持的 %d 新，部分新功能不可用；请升级桌面端。", version, protocolMax), "warning"
	}
	return "", ""
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
	if d.bridgeFailed {
		v.Draft, v.Attachments = text, attachments
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
		if d.messageQueue {
			d.enqueuePrompt(v, text, "steer", attachments)
			return
		}
		v.Queue = append(v.Queue, queuedPrompt{Text: text, Attachments: attachments})
		return
	}
	v.sendGeneration++
	generation := v.sendGeneration
	previousRun := v.State.RunID
	v.stateVersion++
	v.Error = ""
	v.Busy = true
	v.AwaitingRun = true
	v.CancelPending = false
	v.Loaded = true
	v.StartedAt = time.Now()
	d.rememberPrompt(text)
	user := &model.FeedItem{ID: fmt.Sprint("user-", d.next.Add(1)), Kind: "user", Text: text, Attachments: attachments, StartedAt: v.StartedAt}
	v.append(user,
		&model.FeedItem{ID: fmt.Sprint("thinking-", d.next.Add(1)), Kind: "thinking", Text: "正在思考", StartedAt: v.StartedAt})
	var result struct {
		SessionID string `json:"sessionId"`
		RunID     string `json:"runId"`
	}
	d.request(v, "sendPrompt", map[string]any{"text": text, "model": v.State.Model, "attachments": attachments, "asyncStart": true}, &result, func(err error) {
		if generation != v.sendGeneration {
			return
		}
		rollback := func(err error) {
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
			v.AwaitingRun = false
			d.error(v, err)
		}
		if err != nil {
			if !errors.Is(err, context.DeadlineExceeded) {
				rollback(err)
				return
			}
			// A timeout does not revoke a request already accepted by the agent.
			// A run_started/terminal may have settled it before this callback.
			if !v.AwaitingRun && v.State.RunID != previousRun {
				return
			}
			var state model.State
			d.request(v, "getState", nil, &state, func(stateErr error) {
				if generation != v.sendGeneration || (!v.AwaitingRun && v.State.RunID != previousRun) {
					return
				}
				if stateErr != nil {
					v.Error = "发送状态尚未确认：" + stateErr.Error()
					return
				}
				if state.Running || (state.RunID != "" && state.RunID != previousRun) {
					v.State = state
					v.SessionID = state.SessionID
					v.Busy = state.Running
					v.AwaitingRun = false
					if !state.Running {
						v.removeThinking()
						v.StartedAt = time.Time{}
					}
					return
				}
				rollback(err)
			})
			return
		}
		v.SessionID = result.SessionID
		v.State.RunID = result.RunID
		v.AwaitingRun = false
		d.refreshSessions(v)
	})
}

func (d *desktop) submit() {
	v := d.current()
	if v.Uploading || v.Loading {
		return
	}
	// An empty Enter while a task runs (a stray key, an IME commit) does
	// nothing; cancelling is bound to the stop button and Cmd+. only.
	if v.Busy && strings.TrimSpace(v.Draft) == "" && len(v.Attachments) == 0 {
		return
	}
	text := v.Draft
	attachments := slices.Clone(v.Attachments)
	if strings.TrimSpace(text) == "" && len(attachments) == 0 && len(v.Queue) > 0 && !v.Busy {
		if d.messageQueue {
			d.request(v, "resumeQueue", nil, nil, func(err error) { d.error(v, err); d.refreshServerQueue(v) })
			return
		}
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
	scope := ""
	if remember {
		scope = "session"
	}
	d.decideScope(item, granted, scope)
}
func (d *desktop) decideScope(item *model.FeedItem, granted bool, scope string) {
	v := d.current()
	id := item.ApprovalID
	if id == "" || item.Deciding {
		return
	}
	item.Deciding = true
	params := map[string]any{"approvalId": id, "granted": granted}
	if granted && scope != "" {
		params["remember"] = scope
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
	if slices.Contains([]string{"/clear", "/new", "/cls"}, strings.ToLower(command[0])) && len(command) != 1 {
		v.Error = command[0] + " 不接受参数"
		return
	}
	switch command[0] {
	case "/steer", "/follow":
		if !d.messageQueue {
			v.Error = "Agent does not support message queues"
			return
		}
		mode := "steer"
		if command[0] == "/follow" {
			mode = "followUp"
		}
		d.enqueuePrompt(v, strings.TrimSpace(strings.TrimPrefix(text, command[0])), mode, nil)
		return
	case "/queue":
		if !(len(command) == 1 || len(command) == 2 && command[1] == "resume" || len(command) == 3 && command[1] == "cancel") {
			v.Error = "用法：/queue [resume | cancel ID]"
			return
		}
		if len(command) == 3 && command[1] == "cancel" {
			d.request(v, "deleteQueuedPrompt", map[string]any{"id": command[2]}, nil, func(err error) { d.error(v, err); d.refreshServerQueue(v) })
		} else if len(command) == 2 && command[1] == "resume" {
			d.request(v, "resumeQueue", nil, nil, func(err error) { d.error(v, err); d.refreshServerQueue(v) })
		} else {
			d.refreshServerQueue(v)
		}
		return
	case "/cls":
		v.Items = nil
		v.History = nil
		v.HistoryCount = 0
		v.rebuildRows()
		return
	case "/new", "/clear":
		if v.Busy {
			v.Error = "当前回合结束后才能执行命令"
			return
		}
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
	case "/operator":
		tail := strings.TrimSpace(strings.TrimPrefix(text, command[0]))
		if tail == "" {
			v.append(&model.FeedItem{ID: fmt.Sprint("operator-", d.next.Add(1)), Kind: "notice", Text: tr("用法：/operator <需要子代理并行编排的任务描述>")})
			return
		}
		d.sendAttached(tr("请使用子助手编排并执行以下任务：")+tail, v, nil)
		return
	case "/help":
		var text strings.Builder
		for _, cmd := range d.commands {
			fmt.Fprintf(&text, "%s  %s\n", cmd.Usage, cmd.Summary)
		}
		v.append(&model.FeedItem{ID: fmt.Sprint("help-", d.next.Add(1)), Kind: "notice", Text: text.String()})
		return
	}
	if d.bridgeFailed {
		if v.Draft == "" {
			v.Draft = text
		}
		return
	}
	var result struct {
		Action    string       `json:"action"`
		Message   string       `json:"message"`
		SessionID string       `json:"sessionId"`
		State     *model.State `json:"state"`
	}
	continuing := command[0] == "/continue" && !v.Busy
	if command[0] == "/resume" {
		d.navigationVersion++
	}
	navigation := d.navigationVersion
	if continuing {
		v.AwaitingRun = true
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
			v.History = nil
			v.HistoryCount = 0
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
			if navigation == d.navigationVersion && d.active == v.Key {
				d.resumeSession(v, result.SessionID, navigation)
			}
		}
	})
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
