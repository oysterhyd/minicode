import { useEffect, useRef, useState, type SetStateAction } from 'react'
import type { AgentEvent, AgentState, Capabilities, Change, Command, DesktopEvent, FeedItem, Model, Session, SessionDetail, SessionTask } from '../types'
import { historyItems } from '../types'

export const initialState: AgentState = { model: 'fake', effort: 'off', permissionMode: 'default', sessionId: null,
  taskPending: false, rounds: 0, contextTokens: 0, contextWindow: 200000, usage: null,
  budget: { max_rounds: 0, max_total_tokens: 0, max_seconds: 0 }, acceptance: '' }
type View = {
  sessionId: string | null; workspace: string | null; agentState: AgentState; items: FeedItem[]; trace: AgentEvent[];
  tasks: SessionTask[]; changes: Change[]; files: string[]; capabilities: Capabilities | null; draft: string; queue: string[];
  busy: boolean; loading: boolean; loaded: boolean; error: string; startedAt: number | null; unread: boolean;
}
const emptyView = (workspace: string | null = null, state = initialState): View => ({ sessionId: null, workspace,
  agentState: { ...state, sessionId: null, usage: null, rounds: 0, contextTokens: 0, taskPending: false, acceptance: '' },
  items: [], trace: [], tasks: [], changes: [], files: [], capabilities: null, draft: '', queue: [], busy: false,
  loading: false, loaded: false, error: '', startedAt: null, unread: false })

type Stream = { id: string; text: string; timer: ReturnType<typeof setTimeout> | null }

export function useConversations() {
  const [views, setViews] = useState<Record<string, View>>({ 'draft-0': emptyView() })
  const viewsRef = useRef(views)
  const [activeKey, setActiveKey] = useState('draft-0')
  const activeRef = useRef(activeKey)
  const [sessions, setSessions] = useState<Session[]>([])
  const [models, setModels] = useState<Model[]>([])
  const [commands, setCommands] = useState<Command[]>([])
  const streams = useRef(new Map<string, Stream>())
  const progressTimers = useRef(new Map<string, ReturnType<typeof setTimeout>>())
  const versions = useRef(new Map<string, number>())
  const sessionVersion = useRef(0)
  const sendRef = useRef<(text: string, key: string) => Promise<void>>(async () => {})
  const mounted = useRef(true)

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
  function fail(key: string, error: unknown) { update(key, { error: String(error) }) }
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
        [...view.items, { id: stream.id, kind: 'assistant', text, pending: true }] }
    })
  }
  function onEvent(event: DesktopEvent) {
    if (event.event === 'bridge_error') {
      Object.keys(viewsRef.current).forEach(key => update(key, view => ({ ...view, error: event.error || 'Agent bridge 出错',
        busy: /exited|不可用|无效消息/.test(event.error || '') ? false : view.busy })))
      return
    }
    const key = Object.keys(viewsRef.current).find(id => event.sessionId && viewsRef.current[id].sessionId === event.sessionId)
      || (event.clientKey && viewsRef.current[event.clientKey] ? event.clientKey : undefined)
    if (!key) return
    if (event.sessionId && !viewsRef.current[key].sessionId) update(key, { sessionId: event.sessionId })
    const setItems = field(key, 'items')
    if (event.event === 'run_done' || event.event === 'run_error') {
      flush(key); streams.current.delete(key)
      const timer = progressTimers.current.get(key)
      if (timer) clearTimeout(timer)
      progressTimers.current.delete(key)
      update(key, view => ({ ...view, busy: false, unread: key !== activeRef.current,
        error: event.event === 'run_error' ? event.error || '执行失败' : view.error,
        items: view.items.filter(item => item.kind !== 'thinking').map(item => ({ ...item,
          ...(item.approvalId ? { approvalId: undefined, granted: false } : {}),
          ...(item.pending ? { pending: false, interrupted: item.kind === 'tool' } : {}) })),
      }))
      if (event.result?.exit_reason && event.result.exit_reason !== 'completed') setItems(items => [...items,
        { id: `notice-${Date.now()}`, kind: 'notice', text: `任务已暂停：${event.result?.error || event.result?.exit_reason}` }])
      void requestFor<AgentState>(key, 'getState').then(agentState => update(key, { agentState })).catch(error => fail(key, error))
      void refresh(key)
      const next = viewsRef.current[key].queue[0]
      if (next) {
        update(key, view => ({ ...view, queue: view.queue.slice(1) }))
        queueMicrotask(() => void sendRef.current(next, key))
      }
      return
    }
    if (event.event === 'text_delta' && event.text) {
      let stream = streams.current.get(key)
      if (!stream) { stream = { id: `stream-${crypto.randomUUID()}`, text: '', timer: null }; streams.current.set(key, stream) }
      stream.text += event.text
      if (!stream.timer) stream.timer = setTimeout(() => flush(key), 50)
    }
    if (event.event === 'approval' && event.request) {
      setItems(items => [...items, { id: `approval-${event.approvalId}`, kind: 'approval', name: event.request!.tool_name,
        args: event.request!.arguments, approvalId: event.approvalId }])
    }
    if (event.event !== 'agent_event' || !event.item) return
    const { type, data, seq } = event.item
    update(key, view => ({ ...view, trace: [...view.trace, event.item!].slice(-200) }))
    if (type === 'round_start') {
      setItems(items => [...items.filter(item => item.kind !== 'thinking'), { id: `thinking-${seq}`, kind: 'thinking', text: '正在思考' }])
      update(key, view => ({ ...view, agentState: { ...view.agentState, rounds: Number(data.round || view.agentState.rounds) } }))
    }
    if (type === 'assistant_message') {
      flush(key)
      const stream = streams.current.get(key); streams.current.delete(key)
      setItems(items => {
        const rest = items.filter(item => item.kind !== 'thinking' && item.id !== stream?.id)
        return data.text ? [...rest, { id: `assistant-${seq}`, kind: 'assistant', text: String(data.text) }] : rest
      })
    }
    if (type === 'tool_call_start') setItems(items => [...items, { id: String(data.call_id), kind: 'tool', name: String(data.name), args: data.arguments as Record<string, unknown>, pending: true }])
    if (type === 'tool_call_result') {
      setItems(items => items.map(item => item.id === String(data.call_id) ? { ...item, pending: false, success: Boolean(data.success),
        output: [data.output_detail || data.output_preview, data.error].filter(Boolean).join('\n'), artifactId: data.artifact_id ? String(data.artifact_id) : undefined } : item))
      if (!progressTimers.current.has(key)) progressTimers.current.set(key, setTimeout(() => {
        progressTimers.current.delete(key); void refreshProgress(key).catch(error => fail(key, error))
      }, 150))
    }
    if (type === 'tool_output') setItems(items => items.map(item => item.id === String(data.call_id) ? { ...item, output: String(data.output_preview || '') } : item))
    if (type === 'approval_decision') setItems(items => items.map(item => item.kind === 'approval' && item.approvalId ? { ...item, approvalId: undefined, granted: Boolean(data.granted) } : item))
    if (type === 'subagent_start') setItems(items => items.map(item => item.kind === 'tool' && item.name === 'delegate' && item.args?.kind === data.kind && item.args?.task === data.task ? { ...item, childSessionId: String(data.child_session_id) } : item))
    const notice = type === 'mcp_discovery' && data.error ? String(data.error) :
      type === 'context_compacted' ? '上下文已压缩' : type === 'goal_check' ? `验收：${data.passed ? '通过' : '仍需改进'}` :
      type === 'background_job_completed' || type === 'background_job_lost' ? `后台命令 ${data.job_id}: ${type === 'background_job_completed' ? '已完成' : '结果丢失'}` :
      type === 'side_effect_unknown' ? `工具执行结果需复核：${data.name || data.call_id || ''}` : null
    if (notice) setItems(items => [...items, { id: `notice-${seq}`, kind: 'notice', text: notice }])
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
    const key = `draft-${crypto.randomUUID()}`
    const previous = viewsRef.current[source]
    viewsRef.current = { ...viewsRef.current, [key]: emptyView(root, previous.agentState) }
    setViews(viewsRef.current); activate(key)
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
    activate(key)
    if (viewsRef.current[key].loaded || viewsRef.current[key].busy) { void refresh(key); return }
    update(key, { loading: true })
    try {
      const agentState = await requestFor<AgentState>(key, 'selectSession')
      const detail = await requestFor<SessionDetail>(key, 'getSession')
      update(key, { agentState, items: historyItems(detail).map(item => item.pending ? { ...item, pending: false, interrupted: true } : item),
        trace: detail.events, loaded: true, error: '' })
      void refresh(key); void loadCapabilities(key)
    } catch (error) { fail(key, error) }
    finally { update(key, { loading: false }) }
  }
  async function sendNow(text: string, key = activeRef.current) {
    const view = viewsRef.current[key]
    if (!view.workspace) { fail(key, '请先选择本地工作区'); return }
    if (view.loading) return
    if (view.busy) { update(key, { queue: [...view.queue, text] }); return }
    update(key, { error: '', busy: true, loaded: true, startedAt: Date.now(), items: [...view.items, { id: `user-${crypto.randomUUID()}`, kind: 'user', text }] })
    try {
      const result = await requestFor<{ sessionId: string }>(key, 'sendPrompt', { text, model: view.agentState.model })
      update(key, { sessionId: result.sessionId })
      void refresh(key)
    } catch (error) { update(key, { error: String(error), busy: false }) }
  }
  sendRef.current = sendNow
  const view = views[activeKey]
  return { ...view, activeKey, views, sessions, models, commands, openSession, newSession, sendNow,
    request: <T = unknown>(method: string, params: Record<string, unknown> = {}) => requestFor<T>(activeKey, method, params),
    refresh: () => refresh(activeKey), loadCapabilities: () => loadCapabilities(activeKey),
    setAgentState: field(activeKey, 'agentState'), setDraft: field(activeKey, 'draft'), setItems: field(activeKey, 'items'),
    setError: field(activeKey, 'error'), setCapabilities: field(activeKey, 'capabilities'),
    setBusy: (value: boolean) => update(activeKey, { busy: value, ...(value ? { startedAt: Date.now() } : {}) }),
    removeQueued: (index: number) => update(activeKey, current => ({ ...current, queue: current.queue.filter((_, i) => i !== index) })),
  }
}
