import { useEffect, useRef, useState, type SetStateAction } from 'react'
import type { AgentEvent, AgentState, Capabilities, Change, Command, DesktopEvent, FeedItem, Model, Session, SessionDetail, SessionTask } from '../types'
import { historyItems } from '../types'

export const initialState: AgentState = { model: '', effort: 'off', permissionMode: 'default', sessionId: null,
  taskPending: false, rounds: 0, contextTokens: 0, contextWindow: 200000, contextBreakdown: { system: 0, tools: 0, messages: 0 }, usage: null,
  budget: { max_rounds: 0, max_total_tokens: 0, max_seconds: 0 }, acceptance: '', alwaysAllow: [] }
export type View = {
  sessionId: string | null; workspace: string | null; agentState: AgentState; items: FeedItem[]; trace: AgentEvent[];
  tasks: SessionTask[]; changes: Change[]; files: string[]; capabilities: Capabilities | null; draft: string; queue: string[];
  busy: boolean; loading: boolean; loaded: boolean; error: string; startedAt: number | null; unread: boolean;
}
const emptyView = (workspace: string | null = null, state = initialState): View => ({ sessionId: null, workspace,
  agentState: { ...state, sessionId: null, usage: null, statistics: undefined, rounds: 0, contextTokens: 0, contextBreakdown: { system: 0, tools: 0, messages: 0 }, taskPending: false, acceptance: '', alwaysAllow: [] },
  items: [], trace: [], tasks: [], changes: [], files: [], capabilities: null, draft: '', queue: [], busy: false,
  loading: false, loaded: false, error: '', startedAt: null, unread: false })

type Stream = { id: string; text: string; timer: ReturnType<typeof setTimeout> | null }
/** Something the shell may want to surface (toast / system notification). */
export type Signal =
  | { type: 'done'; key: string; sessionId: string | null; title: string; ok: boolean; reason?: string }
  | { type: 'approval'; key: string; sessionId: string | null; title: string; tool: string }
  | { type: 'notification_click'; sessionId?: string }

function eventTime(value: unknown) { const parsed = Date.parse(String(value || '')); return Number.isNaN(parsed) ? Date.now() : parsed }

export function useConversations(onSignal?: (signal: Signal) => void) {
  const [views, setViews] = useState<Record<string, View>>({ 'draft-0': emptyView() })
  const viewsRef = useRef(views)
  const [activeKey, setActiveKey] = useState('draft-0')
  const activeRef = useRef(activeKey)
  const [sessions, setSessions] = useState<Session[]>([])
  const sessionsRef = useRef(sessions)
  sessionsRef.current = sessions
  const [models, setModels] = useState<Model[]>([])
  const [commands, setCommands] = useState<Command[]>([])
  const streams = useRef(new Map<string, Stream>())
  const progressTimers = useRef(new Map<string, ReturnType<typeof setTimeout>>())
  const versions = useRef(new Map<string, number>())
  const sessionVersion = useRef(0)
  const mounted = useRef(true)
  const signal = useRef(onSignal)
  signal.current = onSignal

  function update(key: string, change: Partial<View> | ((view: View) => View)) {
    const previous = viewsRef.current[key]
    if (!previous || !mounted.current) return
    const next = typeof change === 'function' ? change(previous) : { ...previous, ...change }
    viewsRef.current = { ...viewsRef.current, [key]: next }
    setViews(viewsRef.current)
  }
  function field<K extends keyof View>(key: string, name: K) {
    return (value: SetStateAction<View[K]>) => update(key, view => ({ ...view,
      [name]: typeof value === 'function' ? (value as (previous: View[K]) => View[K])(view[name]) : value }))
  }
  function activate(key: string) { activeRef.current = key; setActiveKey(key); update(key, { unread: false }) }
  function requestFor<T = unknown>(key: string, method: string, params: Record<string, unknown> = {}) {
    const view = viewsRef.current[key]
    return window.desktop.request<T>(method, { sessionId: view?.sessionId, workspace: view?.workspace, clientKey: key, ...params })
  }
  function fail(key: string, error: unknown) { update(key, { error: String(error).replace(/^Error: /, '') }) }
  function titleOf(key: string) {
    const view = viewsRef.current[key]
    return sessionsRef.current.find(session => session.session_id === view?.sessionId)?.title
      || view?.items.find(item => item.kind === 'user')?.text?.split('\n')[0].slice(0, 72) || '新建任务'
  }
  async function refreshSessions() {
    const version = ++sessionVersion.current
    try {
      const value = await window.desktop.request<Session[]>('listSessions')
      if (mounted.current && version === sessionVersion.current) setSessions(value)
    } catch (error) { fail(activeRef.current, error) }
  }
  async function refreshProgress(key: string) {
    const version = (versions.current.get(key) || 0) + 1
    versions.current.set(key, version)
    const [changes, tasks] = await Promise.all([
      requestFor<Change[]>(key, 'changes'), requestFor<SessionTask[]>(key, 'listTasks'),
    ])
    if (versions.current.get(key) === version) update(key, { changes, tasks })
  }
  async function refresh(key = activeRef.current) {
    await Promise.all([
      refreshSessions(), refreshProgress(key).catch(error => fail(key, error)),
      requestFor<string[]>(key, 'listFiles').then(files => update(key, { files })).catch(error => fail(key, error)),
    ])
  }
  async function refreshConfiguration() {
    const key = activeRef.current
    setModels(await requestFor<Model[]>(key, 'listModels'))
    await loadCapabilities(key)
    const agentState = await requestFor<AgentState>(key, 'getState')
    update(key, { agentState })
  }
  async function loadCapabilities(key = activeRef.current) {
    if (!viewsRef.current[key]?.workspace) return
    try { update(key, { capabilities: await requestFor<Capabilities>(key, 'getCapabilities') }) }
    catch (error) { fail(key, error) }
  }
  function flush(key: string) {
    const stream = streams.current.get(key)
    if (!stream) return
    if (stream.timer) clearTimeout(stream.timer)
    stream.timer = null
    const text = stream.text
    stream.text = ''
    if (!text) return
    update(key, view => {
      const last = view.items.at(-1)
      return { ...view, items: last?.id === stream.id ? [...view.items.slice(0, -1), { ...last, text: (last.text || '') + text }] :
        [...view.items.filter(item => item.kind !== 'thinking'), { id: stream.id, kind: 'assistant', text, pending: true }] }
    })
  }
  function finishRun(key: string, event: DesktopEvent) {
    flush(key); streams.current.delete(key)
    const timer = progressTimers.current.get(key)
    if (timer) clearTimeout(timer)
    progressTimers.current.delete(key)
    const view = viewsRef.current[key]
    const ended = Date.now()
    const reason = event.result?.exit_reason
    const ok = event.event === 'run_done' && (!reason || reason === 'completed')
    const lastUser = view.items.map(item => item.kind).lastIndexOf('user')
    const turnTools = view.items.slice(lastUser + 1).filter(item => item.kind === 'tool').length
    update(key, current => ({ ...current, busy: false, unread: key !== activeRef.current,
      error: event.event === 'run_error' ? event.error || '执行失败' : current.error,
      items: [...current.items.filter(item => item.kind !== 'thinking').map(item => ({ ...item,
        ...(item.approvalId ? { approvalId: undefined, granted: false } : {}),
        ...(item.pending ? { pending: false, interrupted: item.kind === 'tool', endedAt: item.endedAt || ended } : {}) })),
      ...(current.startedAt ? [{ id: `summary-${ended}`, kind: 'summary' as const, startedAt: current.startedAt, endedAt: ended,
        text: String(turnTools), success: ok, interrupted: reason === 'cancelled' }] : []),
      ...(reason && reason !== 'completed' && reason !== 'cancelled' ? [{ id: `notice-${ended}`, kind: 'notice' as const, tone: 'warning' as const,
        text: `任务已暂停：${event.result?.error || reason}${current.agentState.taskPending ? '。可输入 /continue 继续' : ''}` }] : []),
      ] }))
    void requestFor<AgentState>(key, 'getState').then(agentState => update(key, { agentState })).catch(error => fail(key, error))
    void refresh(key)
    signal.current?.({ type: 'done', key, sessionId: view.sessionId, title: titleOf(key), ok,
      reason: event.event === 'run_error' ? event.error : reason })
    const next = viewsRef.current[key].queue[0]
    if (next) {
      update(key, current => ({ ...current, queue: current.queue.slice(1) }))
      queueMicrotask(() => void sendNow(next, key))
    }
  }
  function onEvent(event: DesktopEvent) {
    if (event.event === 'notification_click') { signal.current?.({ type: 'notification_click', sessionId: event.sessionId }); return }
    if (event.event === 'bridge_error') {
      const fatal = /exited|不可用|无效消息/.test(event.error || '')
      // Python writes warnings to stderr too; only surface them when they are real failures.
      if (!fatal && !/Traceback|Error|错误/.test(event.error || '')) return
      Object.keys(viewsRef.current).forEach(key => update(key, view => ({ ...view, error: event.error || 'Agent bridge 出错',
        busy: fatal ? false : view.busy })))
      return
    }
    const clientView = event.clientKey && viewsRef.current[event.clientKey]
    const key = clientView && (!event.sessionId || !clientView.sessionId || clientView.sessionId === event.sessionId)
      ? event.clientKey
      : Object.keys(viewsRef.current).find(id => event.sessionId && viewsRef.current[id].sessionId === event.sessionId)
    if (!key) return
    if (event.sessionId && !viewsRef.current[key].sessionId) update(key, { sessionId: event.sessionId })
    const setItems = field(key, 'items')
    if (event.event === 'run_done' || event.event === 'run_error') { finishRun(key, event); return }
    if (event.event === 'text_delta' && event.text) {
      let stream = streams.current.get(key)
      if (!stream) { stream = { id: `stream-${crypto.randomUUID()}`, text: '', timer: null }; streams.current.set(key, stream) }
      stream.text += event.text
      if (!stream.timer) stream.timer = setTimeout(() => flush(key), 40)
    }
    if (event.event === 'approval' && event.request) {
      flush(key); streams.current.delete(key)
      setItems(items => [...items.filter(item => item.kind !== 'thinking').map(item => item.pending && item.kind === 'assistant' ? { ...item, pending: false } : item), { id: `approval-${event.approvalId}`, kind: 'approval', name: event.request!.tool_name,
        args: event.request!.arguments, approvalId: event.approvalId }])
      signal.current?.({ type: 'approval', key, sessionId: viewsRef.current[key].sessionId, title: titleOf(key), tool: event.request.tool_name })
    }
    if (event.event === 'approval_auto' && event.request) {
      const request = event.request
      setItems(items => {
        const index = items.findLastIndex(item => item.kind === 'tool' && item.pending && item.name === request.tool_name)
        return index < 0 ? items : items.map((item, i) => i === index ? { ...item, auto: true } : item)
      })
    }
    if (event.event !== 'agent_event' || !event.item) return
    const { type, data, seq, timestamp } = event.item
    update(key, view => ({ ...view, trace: [...view.trace, event.item!].slice(-300) }))
    if (type === 'round_start') {
      flush(key)
      setItems(items => [...items.filter(item => item.kind !== 'thinking'), { id: `thinking-${seq}`, kind: 'thinking', text: '正在思考', startedAt: eventTime(timestamp) }])
      update(key, view => ({ ...view, agentState: { ...view.agentState, rounds: Number(data.round || view.agentState.rounds) } }))
    }
    if (type === 'provider_retry') {
      setItems(items => [...items.filter(item => item.kind !== 'thinking'), {
        id: `retry-${seq}`, kind: 'notice', tone: 'warning',
        text: `模型请求暂时失败，${Number(data.delay_s || 0)} 秒后重试（第 ${Number(data.next_attempt || 2)} 次）。`,
      }])
    }
    if (type === 'assistant_message' || type === 'subagent_result' || type === 'context_compacted') {
      void requestFor<AgentState>(key, 'getState').then(agentState => update(key, { agentState })).catch(error => fail(key, error))
    }
    if (type === 'assistant_message') {
      flush(key)
      const stream = streams.current.get(key); streams.current.delete(key)
      setItems(items => {
        const rest = items.filter(item => item.kind !== 'thinking' && item.id !== stream?.id)
        return data.text ? [...rest, { id: `assistant-${seq}`, kind: 'assistant', text: String(data.text) }] : rest
      })
    }
    if (type === 'tool_call_start') {
      flush(key); streams.current.delete(key)
      setItems(items => [...items.filter(item => item.kind !== 'thinking').map(item => item.pending && item.kind === 'assistant' ? { ...item, pending: false } : item),
        { id: String(data.call_id), kind: 'tool', name: String(data.name), args: data.arguments as Record<string, unknown>, pending: true, startedAt: eventTime(timestamp) }])
    }
    if (type === 'tool_call_result') {
      setItems(items => items.map(item => item.id === String(data.call_id) ? { ...item, pending: false, success: Boolean(data.success), endedAt: eventTime(timestamp),
        output: [data.output_detail || data.output_preview, data.error].filter(Boolean).join('\n'), artifactId: data.artifact_id ? String(data.artifact_id) : undefined } : item))
      if (!progressTimers.current.has(key)) progressTimers.current.set(key, setTimeout(() => {
        progressTimers.current.delete(key); void refreshProgress(key).catch(error => fail(key, error))
      }, 150))
    }
    if (type === 'tool_output') setItems(items => items.map(item => item.id === String(data.call_id) ? { ...item, output: String(data.output_preview || '') } : item))
    if (type === 'approval_decision') setItems(items => items.map(item => item.kind === 'approval' && item.approvalId ? { ...item, approvalId: undefined, granted: Boolean(data.granted) } : item))
    if (type === 'subagent_start') setItems(items => items.map(item => item.kind === 'tool' && item.name === 'delegate' && item.args?.kind === data.kind && item.args?.task === data.task ? { ...item, childSessionId: String(data.child_session_id) } : item))
    const notice = type === 'mcp_discovery' && data.error ? String(data.error) :
      type === 'context_compacted' ? '上下文已自动压缩，较早的内容已归档' : type === 'goal_check' ? `验收检查：${data.passed ? '通过' : '仍需改进'}` :
      type === 'background_job_completed' || type === 'background_job_lost' ? `后台命令 ${data.job_id}：${type === 'background_job_completed' ? '已完成' : '结果丢失'}` :
      type === 'side_effect_unknown' ? `工具执行结果需复核：${data.name || data.call_id || ''}` : null
    if (notice) setItems(items => [...items, { id: `notice-${seq}`, kind: 'notice', text: notice,
      tone: type === 'mcp_discovery' || type === 'side_effect_unknown' || type === 'background_job_lost' ? 'warning' : 'info' }])
  }
  const eventRef = useRef(onEvent)
  eventRef.current = onEvent
  useEffect(() => {
    mounted.current = true
    let active = true
    Promise.all([window.desktop.request<string | null>('getWorkspace'), requestFor<{ sessions: Session[]; models: Model[]; commands: Command[]; state: AgentState }>('draft-0', 'initialize')])
      .then(([workspace, initial]) => {
        if (!active) return
        update('draft-0', { workspace, agentState: initial.state, loaded: true })
        setSessions(initial.sessions); setModels(initial.models); setCommands(initial.commands)
        if (workspace) { void refresh('draft-0'); void loadCapabilities('draft-0') }
      }).catch(error => { if (active) fail('draft-0', error) })
    const unsubscribe = window.desktop.onEvent(event => eventRef.current(event))
    return () => {
      active = false; mounted.current = false; unsubscribe()
      for (const stream of streams.current.values()) if (stream.timer) clearTimeout(stream.timer)
      for (const timer of progressTimers.current.values()) clearTimeout(timer)
      streams.current.clear(); progressTimers.current.clear()
    }
  }, [])
  async function newSession(root = viewsRef.current[activeRef.current].workspace) {
    const source = activeRef.current
    // Reuse an untouched draft for the same workspace instead of piling up empty views.
    const reusable = Object.keys(viewsRef.current).find(id => {
      const view = viewsRef.current[id]
      return !view.sessionId && !view.busy && view.items.length === 0 && view.workspace === root && id !== source
    })
    const key = reusable || `draft-${crypto.randomUUID()}`
    const previous = viewsRef.current[source]
    if (!reusable) { viewsRef.current = { ...viewsRef.current, [key]: emptyView(root, previous.agentState) }; setViews(viewsRef.current) }
    activate(key)
    try {
      const agentState = await requestFor<AgentState>(key, 'resetSession', { sourceClientKey: source })
      update(key, { agentState, loaded: true })
      if (root) {
        await requestFor(key, 'setWorkspace', { workspace: root })
        void refresh(key); void loadCapabilities(key)
      }
    } catch (error) { fail(key, error) }
  }
  async function openSession(session: Session) {
    let key = Object.keys(viewsRef.current).find(id => viewsRef.current[id].sessionId === session.session_id)
    if (!key) {
      key = session.session_id
      viewsRef.current = { ...viewsRef.current, [key]: { ...emptyView(session.workspace), sessionId: session.session_id } }
      setViews(viewsRef.current)
    }
    // Remember which conversation the user is leaving: its Bridge holds the
    // permission mode, budget, effort and model they are working with.
    const source = activeRef.current
    activate(key)
    if (viewsRef.current[key].loaded || viewsRef.current[key].busy) { void refresh(key); return }
    update(key, { loading: true })
    try {
      const agentState = await requestFor<AgentState>(key, 'selectSession', { sourceClientKey: source })
      const detail = await requestFor<SessionDetail>(key, 'getSession')
      update(key, { agentState, items: historyItems(detail).map(item => item.pending ? { ...item, pending: false, interrupted: true } : item),
        trace: detail.events.slice(-300), loaded: true, error: '' })
      void refresh(key); void loadCapabilities(key)
    } catch (error) { fail(key, error) }
    finally { update(key, { loading: false }) }
  }
  async function sendNow(text: string, key = activeRef.current) {
    const view = viewsRef.current[key]
    if (!view.workspace) { fail(key, '请先选择本地工作区'); return }
    if (view.loading) return
    if (view.busy) { update(key, { queue: [...view.queue, text] }); return }
    const now = Date.now()
    update(key, { error: '', busy: true, loaded: true, startedAt: now, items: [...view.items,
      { id: `user-${crypto.randomUUID()}`, kind: 'user', text, startedAt: now },
      { id: `thinking-start-${now}`, kind: 'thinking', text: '正在思考', startedAt: now }] })
    try {
      const result = await requestFor<{ sessionId: string }>(key, 'sendPrompt', { text, model: view.agentState.model })
      update(key, { sessionId: result.sessionId })
      void refreshSessions()
    } catch (error) {
      update(key, current => ({ ...current, error: String(error).replace(/^Error: /, ''), busy: false, startedAt: null,
        items: current.items.filter(item => item.kind !== 'thinking') }))
    }
  }
  function dropSessionViews(sessionId: string) {
    const keys = Object.keys(viewsRef.current).filter(id => viewsRef.current[id].sessionId === sessionId)
    if (!keys.length) return
    const next = { ...viewsRef.current }
    for (const key of keys) delete next[key]
    if (!Object.keys(next).length || keys.includes(activeRef.current)) {
      const workspace = viewsRef.current[activeRef.current]?.workspace ?? null
      const fallback = `draft-${crypto.randomUUID()}`
      next[fallback] = emptyView(workspace, viewsRef.current[activeRef.current]?.agentState)
      viewsRef.current = next; setViews(next); activate(fallback)
      void requestFor<AgentState>(fallback, 'resetSession').then(agentState => update(fallback, { agentState, loaded: true }))
        .then(() => workspace ? refresh(fallback) : undefined).catch(error => fail(fallback, error))
      return
    }
    viewsRef.current = next; setViews(next)
  }
  async function renameSession(sessionId: string, title: string) {
    setSessions(await window.desktop.request<Session[]>('renameSession', { sessionId, title }))
  }
  async function pinSession(sessionId: string, pinned: boolean) {
    setSessions(await window.desktop.request<Session[]>('pinSession', { sessionId, pinned }))
  }
  async function deleteSession(sessionId: string) {
    const running = Object.values(viewsRef.current).some(view => view.sessionId === sessionId && view.busy)
    if (running) throw new Error('任务运行中，无法删除会话')
    setSessions(await window.desktop.request<Session[]>('deleteSession', { sessionId }))
    dropSessionViews(sessionId)
  }
  function activateKey(key: string) { if (viewsRef.current[key]) activate(key) }

  const view = views[activeKey]
  return { ...view, activeKey, views, sessions, models, commands, openSession, newSession, sendNow,
    renameSession, pinSession, deleteSession, activateKey, refreshSessions, refreshConfiguration,
    request: <T = unknown>(method: string, params: Record<string, unknown> = {}) => requestFor<T>(activeKey, method, params),
    refresh: () => refresh(activeKey), loadCapabilities: () => loadCapabilities(activeKey),
    setAgentState: field(activeKey, 'agentState'), setDraft: field(activeKey, 'draft'), setItems: field(activeKey, 'items'),
    setError: field(activeKey, 'error'), setCapabilities: field(activeKey, 'capabilities'),
    setBusy: (value: boolean) => update(activeKey, { busy: value, ...(value ? { startedAt: Date.now() } : {}) }),
    removeQueued: (index: number) => update(activeKey, current => ({ ...current, queue: current.queue.filter((_, i) => i !== index) })),
  }
}
