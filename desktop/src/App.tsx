import { useEffect, useRef, useState } from 'react'
import { ArrowUp, AtSign, ChevronDown, FolderOpen, Plus, Shield, Square, SquarePen } from 'lucide-react'
import { Sidebar } from './components/Sidebar'
import { SessionFeed } from './components/SessionFeed'
import { SettingsPanel, type SettingsTab } from './components/SettingsPanel'
import { WorkPanel } from './components/WorkPanel'
import type { AgentState, Capabilities, Change, Command, DesktopEvent, FeedItem, Model, Session, SessionDetail, SessionTask, SlashResult } from './types'
import { historyItems } from './types'

const initialState: AgentState = { model: 'fake', effort: 'off', permissionMode: 'default', sessionId: null,
  taskPending: false, rounds: 0, contextTokens: 0, contextWindow: 200000, usage: null,
  budget: { max_rounds: 0, max_total_tokens: 0, max_seconds: 0 }, acceptance: '' }
type McpDiscovery = { servers: Array<{ server: string; protocol: string | null; server_version: string | null; error: string | null }>; tools: string[] }
type Suggestion = { value: string; detail: string; kind: 'command' | 'value' }

export default function App() {
  const [workspace, setWorkspace] = useState<string | null>(null)
  const [sessions, setSessions] = useState<Session[]>([])
  const [sessionId, setSessionId] = useState<string | null>(null)
  const [models, setModels] = useState<Model[]>([])
  const [commands, setCommands] = useState<Command[]>([])
  const [agentState, setAgentState] = useState<AgentState>(initialState)
  const [capabilities, setCapabilities] = useState<Capabilities | null>(null)
  const [mcpDiscovery, setMcpDiscovery] = useState<McpDiscovery | null>(null)
  const [settingsOpen, setSettingsOpen] = useState(false)
  const [settingsTab, setSettingsTab] = useState<SettingsTab>('general')
  const [leftCollapsed, setLeftCollapsed] = useState(false)
  const [rightCollapsed, setRightCollapsed] = useState(false)
  const [items, setItems] = useState<FeedItem[]>([])
  const [trace, setTrace] = useState<DesktopEvent['item'][]>([])
  const [changes, setChanges] = useState<Change[]>([])
  const [tasks, setTasks] = useState<SessionTask[]>([])
  const [files, setFiles] = useState<string[]>([])
  const [draft, setDraft] = useState('')
  const [busy, setBusy] = useState(false)
  const [queue, setQueue] = useState<string[]>([])
  const [error, setError] = useState('')
  const [mentionOpen, setMentionOpen] = useState(false)
  const [modelMenuOpen, setModelMenuOpen] = useState(false)
  const [permissionMenuOpen, setPermissionMenuOpen] = useState(false)
  const [slashIndex, setSlashIndex] = useState(0)
  const sessionRef = useRef<string | null>(null)
  const busyRef = useRef(false)
  const queueRef = useRef<string[]>([])
  const sendNowRef = useRef<(text: string) => void>(() => {})
  const refreshRef = useRef<() => void>(() => {})
  const applyStateRef = useRef<(state: AgentState) => void>(() => {})
  const streamId = useRef('')
  const textarea = useRef<HTMLTextAreaElement>(null)

  function applyState(state: AgentState) { setAgentState(state) }
  applyStateRef.current = applyState

  function refresh() {
    window.desktop.request<Session[]>('listSessions').then(setSessions).catch(e => setError(String(e)))
    window.desktop.request<Change[]>('changes').then(setChanges).catch(e => setError(String(e)))
    window.desktop.request<string[]>('listFiles').then(setFiles).catch(e => setError(String(e)))
    if (sessionRef.current) window.desktop.request<SessionTask[]>('listTasks', { sessionId: sessionRef.current }).then(setTasks).catch(e => setError(String(e)))
    else setTasks([])
  }
  refreshRef.current = refresh

  function loadCapabilities(root: string) {
    window.desktop.request<Capabilities>('getCapabilities', { workspace: root }).then(setCapabilities).catch(e => setError(String(e)))
  }

  useEffect(() => {
    Promise.all([
      window.desktop.request<string | null>('getWorkspace'),
      window.desktop.request<{ sessions: Session[]; models: Model[]; defaultModel: string; commands: Command[]; state: AgentState }>('initialize'),
    ]).then(([root, initial]) => {
      setWorkspace(root); setSessions(initial.sessions); setModels(initial.models); setCommands(initial.commands)
      applyStateRef.current(initial.state)
      if (root) { refreshRef.current(); window.desktop.request<Capabilities>('getCapabilities', { workspace: root }).then(setCapabilities).catch(e => setError(String(e))) }
    }).catch(e => setError(String(e)))

    return window.desktop.onEvent((event: DesktopEvent) => {
      if (event.event === 'bridge_error') { setError(event.error || 'Agent bridge 出错'); return }
      if (event.event === 'run_error') {
        setError(event.error || '执行失败'); setBusy(false); busyRef.current = false
        setItems(previous => previous.map(item => item.approvalId ? { ...item, approvalId: undefined, granted: false } : item))
        refreshRef.current(); return
      }
      if (event.event === 'run_done') {
        setBusy(false); busyRef.current = false; refreshRef.current()
        setItems(previous => previous.map(item => item.approvalId ? { ...item, approvalId: undefined, granted: false } : item))
        window.desktop.request<AgentState>('getState').then(applyStateRef.current).catch(e => setError(String(e)))
        if (event.result?.exit_reason && event.result.exit_reason !== 'completed') {
          setItems(previous => [...previous, { id: `notice-${Date.now()}`, kind: 'notice', text: `任务已暂停：${event.result?.error || event.result?.exit_reason}` }])
        }
        const next = queueRef.current.shift()
        setQueue([...queueRef.current])
        if (next) setTimeout(() => sendNowRef.current(next), 0)
        return
      }
      if (event.event === 'agent_event' && event.item?.type === 'session_start' && event.sessionId) {
        sessionRef.current = event.sessionId; setSessionId(event.sessionId)
      }
      if (sessionRef.current && event.sessionId !== sessionRef.current) return
      if (event.event === 'agent_event' && event.item) setTrace(previous => [...previous, event.item!].slice(-200))
      if (event.event === 'text_delta' && event.text) {
        const id = streamId.current || (streamId.current = `stream-${Date.now()}`)
        setItems(previous => {
          const index = previous.findIndex(item => item.id === id)
          if (index < 0) return [...previous, { id, kind: 'assistant', text: event.text, pending: true }]
          return previous.map(item => item.id === id ? { ...item, text: (item.text || '') + event.text } : item)
        })
      }
      if (event.event === 'approval' && event.request) {
        setItems(previous => [...previous, { id: `approval-${event.approvalId}`, kind: 'approval', name: event.request!.tool_name,
          args: event.request!.arguments, approvalId: event.approvalId }])
      }
      if (event.event === 'agent_event' && event.item) {
        const { type, data, seq } = event.item
        if (type === 'round_start') setItems(previous => [...previous, { id: `thinking-${seq}`, kind: 'thinking', text: 'Thinking…' }])
        if (type === 'assistant_message') {
          const id = streamId.current; streamId.current = ''
          setItems(previous => {
            const withoutStream = previous.filter(item => item.kind !== 'thinking' && item.id !== id)
            return data.text ? [...withoutStream, { id: `assistant-${seq}`, kind: 'assistant', text: String(data.text) }] : withoutStream
          })
        }
        if (type === 'tool_call_start') setItems(previous => [...previous, { id: String(data.call_id), kind: 'tool', name: String(data.name),
          args: data.arguments as Record<string, unknown>, pending: true }])
        if (type === 'tool_call_result') {
          setItems(previous => previous.map(item => item.id === String(data.call_id) ? { ...item, pending: false,
            success: Boolean(data.success), output: String(data.output_detail || data.error || ''),
            artifactId: data.artifact_id ? String(data.artifact_id) : undefined } : item))
          refreshRef.current()
        }
        if (type === 'tool_output') setItems(previous => previous.map(item => item.id === String(data.call_id) ? { ...item, output: String(data.output_preview || '') } : item))
        if (type === 'approval_decision') setItems(previous => previous.map(item => item.kind === 'approval' && item.approvalId ? { ...item, approvalId: undefined, granted: Boolean(data.granted) } : item))
        if (type === 'subagent_start') setItems(previous => previous.map(item => item.kind === 'tool' && item.name === 'delegate' && item.args?.kind === data.kind && item.args?.task === data.task ? { ...item, childSessionId: String(data.child_session_id) } : item))
        if (type === 'mcp_discovery' && data.error) setItems(previous => [...previous, { id: `mcp-${seq}`, kind: 'notice', text: String(data.error) }])
        if (type === 'context_compacted') setItems(previous => [...previous, { id: `compact-${seq}`, kind: 'notice', text: 'Context compacted' }])
        if (type === 'goal_check') setItems(previous => [...previous, { id: `goal-${seq}`, kind: 'notice', text: `Goal check: ${data.passed ? 'passed' : 'needs work'}` }])
        if (type === 'background_job_completed' || type === 'background_job_lost') setItems(previous => [...previous, { id: `job-${seq}`, kind: 'notice', text: `Background job ${String(data.job_id || '')}: ${type === 'background_job_completed' ? 'completed' : 'lost'}` }])
        if (type === 'side_effect_unknown') setItems(previous => [...previous, { id: `unknown-${seq}`, kind: 'notice', text: `Tool side effect needs review: ${String(data.name || data.call_id || '')}` }])
      }
    })
  }, [])

  async function bindWorkspace(root: string) {
    if (busyRef.current) return
    await window.desktop.request('setWorkspace', { workspace: root })
    await window.desktop.request('resetSession')
    applyState(await window.desktop.request<AgentState>('setAcceptance', { path: '' }))
    setWorkspace(root); sessionRef.current = null; setSessionId(null); setItems([]); setTrace([]); setError(''); setMcpDiscovery(null)
    setTasks([])
    refresh(); loadCapabilities(root)
  }
  async function chooseWorkspace() { if (busyRef.current) return; const root = await window.desktop.chooseWorkspace(); if (root) await bindWorkspace(root) }

  async function openSession(session: Session) {
    if (busyRef.current) return
    if (workspace !== session.workspace) await bindWorkspace(session.workspace)
    const detail = await window.desktop.request<SessionDetail>('getSession', { sessionId: session.session_id })
    sessionRef.current = session.session_id; setSessionId(session.session_id)
    setItems(historyItems(detail)); setTrace(detail.events); setError('')
    setTasks(await window.desktop.request<SessionTask[]>('listTasks', { sessionId: session.session_id }))
    try { applyState(await window.desktop.request<AgentState>('selectSession', { sessionId: session.session_id })) }
    catch (e) { setError(String(e)) }
    loadCapabilities(session.workspace)
  }

  async function newSession() {
    if (busyRef.current) return
    applyState(await window.desktop.request<AgentState>('resetSession'))
    sessionRef.current = null; setSessionId(null); setItems([]); setTrace([]); setTasks([]); setError(''); textarea.current?.focus()
  }

  async function sendNow(text: string) {
    if (!workspace) { setError('请先选择本地工作区'); return }
    setError(''); setBusy(true); busyRef.current = true
    setItems(previous => [...previous, { id: `user-${Date.now()}`, kind: 'user', text }])
    try {
      const result = await window.desktop.request<{ sessionId: string }>('sendPrompt', { text, workspace, sessionId: sessionRef.current, model: agentState.model })
      sessionRef.current = result.sessionId; setSessionId(result.sessionId); refresh()
    } catch (e) { setError(String(e)); setBusy(false); busyRef.current = false }
  }
  sendNowRef.current = sendNow

  async function changeModel(value: string) {
    try { applyState(await window.desktop.request<AgentState>('setModel', { model: value })); setModelMenuOpen(false); setError('') }
    catch (e) { setError(String(e)) }
  }
  async function changeEffort(value: string) {
    try { applyState(await window.desktop.request<AgentState>('setEffort', { effort: value })); setError('') }
    catch (e) { setError(String(e)) }
  }
  async function changePermission(value: string) {
    try { applyState(await window.desktop.request<AgentState>('setPermissionMode', { mode: value })); setPermissionMenuOpen(false); setError('') }
    catch (e) { setError(String(e)) }
  }
  async function changeBudget(value: AgentState['budget']) {
    try { applyState(await window.desktop.request<AgentState>('setBudget', value)); setError('') }
    catch (e) { setError(String(e)) }
  }
  async function changeAcceptance(path: string) {
    try { applyState(await window.desktop.request<AgentState>('setAcceptance', { path })); setError('') }
    catch (e) { setError(String(e)) }
  }
  async function changeSkill(name: string, active: boolean) {
    if (!workspace) return
    try {
      await window.desktop.request('setSkillActive', { name, active, workspace, sessionId })
      loadCapabilities(workspace); setError('')
    } catch (e) { setError(String(e)) }
  }
  async function changePlugin(name: string, enabled: boolean) {
    if (!workspace) return
    try { setCapabilities(await window.desktop.request<Capabilities>('setPluginEnabled', { name, enabled, workspace })); setMcpDiscovery(null); setError('') }
    catch (e) { setError(String(e)) }
  }
  async function refreshMcp() {
    if (!workspace) return
    try { setMcpDiscovery(await window.desktop.request<McpDiscovery>('refreshMcp', { workspace })); setError('') }
    catch (e) { setError(String(e)) }
  }

  async function executeSlash(text: string) {
    if (busyRef.current) { setError('当前回合结束后再执行命令'); return }
    try {
      const result = await window.desktop.request<SlashResult>('runSlash', { text, workspace, sessionId: sessionRef.current })
      if (result.state) applyState(result.state)
      if (result.action === 'new') await newSession()
      if (result.action === 'clear') setItems([])
      if (result.action === 'exit') { await window.desktop.request('closeWindow'); return }
      if (result.action === 'resume' && result.sessionId) {
        const selected = sessions.find(session => session.session_id === result.sessionId)
        if (selected) await openSession(selected)
      }
      if (result.action === 'continue') { setBusy(true); busyRef.current = true }
      if (result.action === 'skills') { setSettingsTab('skills'); setSettingsOpen(true) }
      if (result.action === 'model' || result.action === 'effort' || result.action === 'permissions' || result.action === 'sessions') {
        setDraft(result.action === 'sessions' ? '/resume ' : `/${result.action} `); setSlashIndex(0); textarea.current?.focus()
      }
      if (result.message && result.action !== 'new' && result.action !== 'clear') setItems(previous => [...previous, { id: `system-${Date.now()}`, kind: 'notice', text: result.message }])
      setError('')
    } catch (e) { setError(String(e)) }
  }

  function submit() {
    const text = draft.trim()
    if (!text) return
    setDraft(''); setMentionOpen(false)
    if (text.startsWith('/')) { void executeSlash(text); return }
    if (busyRef.current) { queueRef.current.push(text); setQueue([...queueRef.current]) }
    else void sendNow(text)
  }

  async function decide(approvalId: string, granted: boolean) {
    try { await window.desktop.request('resolveApproval', { approvalId, granted }) }
    catch (e) { setError(String(e)) }
  }

  const mention = draft.match(/@([\w./-]*)$/)?.[1]
  const fileSuggestions = mentionOpen && mention !== undefined && !draft.startsWith('/') ? files.filter(file => file.toLowerCase().includes(mention.toLowerCase())).slice(0, 7) : []
  function insertFile(file: string) { setDraft(value => value.replace(/@[\w./-]*$/, `@${file} `)); setMentionOpen(false); textarea.current?.focus() }

  let slashOptions: Suggestion[] = []
  if (draft.startsWith('/')) {
    const verb = draft.split(/\s/, 1)[0].toLowerCase()
    const hasSpace = draft.includes(' ')
    const query = hasSpace ? draft.slice(draft.indexOf(' ') + 1).toLowerCase() : ''
    const submenu = hasSpace || ['/model', '/effort', '/permissions', '/resume', '/skill'].includes(verb) && draft === verb
    if (submenu && ['/model', '/effort', '/permissions', '/resume', '/skill'].includes(verb)) {
      const values = verb === '/model' ? models.map(item => ({ value: item.id, detail: item.available ? item.provider : '未配置' })) :
        verb === '/effort' ? ['off', 'low', 'medium', 'high', 'xhigh', 'max'].map(value => ({ value, detail: 'reasoning effort' })) :
        verb === '/permissions' ? ['default', 'accept_edits', 'bypass'].map(value => ({ value, detail: 'permission mode' })) :
        verb === '/resume' ? sessions.slice(0, 20).map(item => ({ value: item.session_id.slice(0, 8), detail: item.title })) :
        capabilities?.skills.map(item => ({ value: item.name, detail: item.description })) || []
      slashOptions = values.filter(item => item.value.toLowerCase().includes(query)).slice(0, 8).map(item => ({ ...item, kind: 'value' }))
    } else {
      slashOptions = commands.filter(command => command.name.startsWith(verb)).slice(0, 8).map(command => ({ value: command.name, detail: command.summary, kind: 'command' }))
    }
  }
  const optionIndex = Math.min(slashIndex, Math.max(0, slashOptions.length - 1))
  function chooseSlash(option: Suggestion) {
    if (option.kind === 'command' && ['/model', '/effort', '/permissions', '/resume', '/skill'].includes(option.value)) {
      setDraft(option.value + ' '); setSlashIndex(0); textarea.current?.focus(); return
    }
    const command = option.kind === 'value' ? `${draft.split(/\s/, 1)[0]} ${option.value}` : option.value
    setDraft(''); void executeSlash(command)
  }

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.ctrlKey && event.key.toLowerCase() === 'n') { event.preventDefault(); newSession() }
      if (event.ctrlKey && event.key.toLowerCase() === 'l') { event.preventDefault(); setItems([]) }
      if (event.ctrlKey && event.key.toLowerCase() === 'i') { event.preventDefault(); setSettingsTab('general'); setSettingsOpen(true) }
      if (event.ctrlKey && event.key.toLowerCase() === 'c' && busyRef.current) { event.preventDefault(); void window.desktop.request('cancelTurn') }
      if (event.key === 'Escape' && settingsOpen) { event.preventDefault(); setSettingsOpen(false) }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  })

  const title = sessions.find(session => session.session_id === sessionId)?.title || '新建任务'
  const contextPercent = Math.min(100, Math.round(100 * agentState.contextTokens / Math.max(1, agentState.contextWindow)))
  return <><div className="titlebar-drag" aria-hidden="true" /><div className="app-shell grid text-primary" style={{ gridTemplateColumns: `${leftCollapsed ? '54px' : 'min(292px, 22vw)'} minmax(0, 1fr) ${rightCollapsed ? '54px' : 'min(380px, 30vw)'}` }}>
    <Sidebar workspace={workspace} sessions={sessions} activeSession={sessionId}
      collapsed={leftCollapsed} onToggle={() => setLeftCollapsed(value => !value)}
      onSettings={() => { setSettingsOpen(true); setSettingsTab('general'); if (workspace) loadCapabilities(workspace) }}
      onChoose={chooseWorkspace} onNew={newSession} onWorkspace={bindWorkspace} onSession={openSession} />
    <main className="flex min-w-0 flex-col bg-main">
      <header className="flex h-16 shrink-0 items-center justify-between border-b border-edge px-7">
        <div className="min-w-0"><div className="truncate text-base font-semibold text-white">{title}</div><div className="mt-0.5 flex items-center gap-1.5 text-[11px] text-subtle"><FolderOpen size={12} />{workspace ? workspace.split(/[\\/]/).pop() : '选择工作区开始'}</div></div>
        <button className="icon-button" title="新建任务" aria-label="新建任务" onClick={newSession}><SquarePen size={18} /></button>
      </header>
      <SessionFeed key={sessionId || 'new'} sessionId={sessionId} items={items} busy={busy} onDecide={decide} />
      {error && <div className="mx-7 mb-2 rounded-lg border border-white/20 bg-white/5 px-3 py-2 text-xs text-primary">{error}</div>}
      <div className="shrink-0 px-6 pb-5 pt-2">
        <div className="composer relative mx-auto max-w-[900px]">
          {fileSuggestions.length > 0 && <div className="mention-popover"><div className="section-label px-3 py-2">引用本地文件</div>{fileSuggestions.map(file => <button key={file} onClick={() => insertFile(file)}><AtSign size={14} /><span className="truncate">{file}</span></button>)}</div>}
          {slashOptions.length > 0 && <div className="slash-popover"><div className="section-label px-3 py-2">Commands · ↑↓ 选择 · Tab 补全 · Enter 执行</div>{slashOptions.map((option, index) => <button key={option.value} className={index === optionIndex ? 'slash-option-active' : ''} onClick={() => chooseSlash(option)}><span className="font-mono text-accent">{option.value}</span><span className="truncate text-xs text-subtle">{option.detail}</span></button>)}</div>}
          <textarea ref={textarea} value={draft} onChange={event => { setDraft(event.target.value); setMentionOpen(true); setSlashIndex(0) }}
            onKeyDown={event => {
              if (slashOptions.length > 0 && ['ArrowDown', 'ArrowUp'].includes(event.key)) { event.preventDefault(); setSlashIndex(value => (value + (event.key === 'ArrowDown' ? 1 : -1) + slashOptions.length) % slashOptions.length); return }
              if (slashOptions.length > 0 && ['Tab', 'Enter'].includes(event.key) && !event.shiftKey) { event.preventDefault(); chooseSlash(slashOptions[optionIndex]); return }
              if (event.key === 'Escape') { setModelMenuOpen(false); setPermissionMenuOpen(false); setDraft(''); return }
              if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) { event.preventDefault(); submit() }
            }} placeholder={workspace ? '让 MiniCode 帮你做任何事。输入 / 查看命令，@ 引用文件…' : '先选择本地工作区…'} rows={3} aria-label="任务输入" />
          <div className="flex items-center gap-2 px-3 pb-3">
            <button className="composer-icon" title="引用文件" aria-label="引用文件" onClick={() => { setDraft(value => value + '@'); setMentionOpen(true); textarea.current?.focus() }}><Plus size={19} /></button>
            <div className="relative"><button className="composer-control" onClick={() => { setPermissionMenuOpen(value => !value); setModelMenuOpen(false) }}><Shield size={14} />{agentState.permissionMode}<ChevronDown size={12} /></button>{permissionMenuOpen && <div className="composer-menu">{['default', 'accept_edits', 'bypass'].map(mode => <button key={mode} onClick={() => changePermission(mode)}>{mode}</button>)}</div>}</div>
            <span className="text-xs text-subtle">{queue.length > 0 ? `${queue.length} queued` : 'Enter 发送 · Shift Enter 换行'}</span>
            <div className="ml-auto flex items-center gap-2"><span className="hidden text-xs text-subtle xl:block">{contextPercent}%</span><div className="relative"><button className="composer-control max-w-[220px]" onClick={() => { setModelMenuOpen(value => !value); setPermissionMenuOpen(false) }} title="切换模型"><span className="truncate">{agentState.model}</span><ChevronDown size={13} /></button>{modelMenuOpen && <div className="composer-menu composer-model-menu">{models.map(option => <button key={option.id} onClick={() => changeModel(option.id)}><span className="truncate">{option.id}</span><span className="text-[11px] text-subtle">{option.available ? option.provider : '未配置'}</span></button>)}</div>}</div><button className="send-button" onClick={busy ? () => void window.desktop.request('cancelTurn') : submit} disabled={!busy && !draft.trim()} aria-label={busy ? '停止任务' : '发送任务'}>{busy ? <Square size={15} fill="currentColor" /> : <ArrowUp size={19} />}</button></div>
          </div>
        </div>
      </div>
    </main>
    <WorkPanel workspace={workspace} changes={changes} files={files} items={items} tasks={tasks} state={agentState} trace={trace.filter(Boolean) as NonNullable<DesktopEvent['item']>[]} refresh={refresh}
      collapsed={rightCollapsed} onToggle={() => setRightCollapsed(value => !value)} />
    {settingsOpen && <SettingsPanel tab={settingsTab} setTab={setSettingsTab} onClose={() => setSettingsOpen(false)}
      models={models} state={agentState} capabilities={capabilities} mcpDiscovery={mcpDiscovery} trace={trace.filter(Boolean) as NonNullable<DesktopEvent['item']>[]} error={error}
      onModel={changeModel} onEffort={changeEffort} onPermission={changePermission} onBudget={changeBudget} onAcceptance={changeAcceptance}
      onChooseAcceptance={() => window.desktop.chooseAcceptanceFile()} onSkill={changeSkill}
      onPlugin={changePlugin} onRefreshMcp={refreshMcp} onLockPlugins={async () => { if (workspace) { try { await window.desktop.request('lockPlugins', { workspace }); loadCapabilities(workspace); setError('') } catch (e) { setError(String(e)) } } }}
      onUseAgent={kind => { setDraft(value => `${value ? `${value.trimEnd()}\n` : ''}请使用 ${kind} 子助手调查并报告证据。`); setSettingsOpen(false); textarea.current?.focus() }} />}
  </div></>
}
