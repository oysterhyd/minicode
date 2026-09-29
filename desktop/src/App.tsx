import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import { AnimatePresence, MotionConfig, motion } from 'motion/react'
import { ArrowUp, AtSign, CircleAlert, Cpu, ListPlus, Plus, Search, Shield, Square, SquarePen, X } from 'lucide-react'
import { Sidebar } from './components/Sidebar'
import { SessionFeed } from './components/SessionFeed'
import { SettingsPanel, type SettingsTab } from './components/SettingsPanel'
import { WorkPanel } from './components/WorkPanel'
import { CommandPalette } from './components/CommandPalette'
import { RunStatus } from './components/RunStatus'
import { SelectMenu } from './components/SelectMenu'
import { ContextMeter } from './components/ContextMeter'
import type { AgentState, Capabilities, DesktopEvent, SlashResult } from './types'
import { useConversations } from './hooks/useConversations'

type McpDiscovery = { servers: Array<{ server: string; protocol: string | null; server_version: string | null; error: string | null }>; tools: string[] }
type Suggestion = { value: string; detail: string; kind: 'command' | 'value' }

function savedPreference(key: string) { try { return localStorage.getItem(`minicode.${key}`) } catch { return null } }
function savePreference(key: string, value: string) { try { localStorage.setItem(`minicode.${key}`, value) } catch { /* Preferences are optional. */ } }

export default function App() {
  const { workspace, sessions, sessionId, models, commands, agentState, capabilities, items, trace, changes, tasks, files,
    draft, busy, loading, queue, error, activeKey, views, startedAt, setAgentState, setCapabilities, setDraft, setItems, setError,
    setBusy, removeQueued, refresh, loadCapabilities, request, openSession, sendNow, newSession: createSession } = useConversations()
  const [discoveries, setDiscoveries] = useState<Record<string, McpDiscovery | null>>({})
  const mcpDiscovery = discoveries[activeKey] || null
  function setMcpDiscovery(value: McpDiscovery | null) { setDiscoveries(previous => ({ ...previous, [activeKey]: value })) }
  const [settingsOpen, setSettingsOpen] = useState(false)
  const [settingsTab, setSettingsTab] = useState<SettingsTab>('general')
  const [leftCollapsed, setLeftCollapsed] = useState(() => savedPreference('left-collapsed') === 'true')
  const [rightCollapsed, setRightCollapsed] = useState(() => savedPreference('right-collapsed') === 'true')
  const [theme, setTheme] = useState<'light' | 'dark'>(() => {
    const saved = savedPreference('theme')
    return saved === 'light' || saved === 'dark' ? saved : window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light'
  })
  const [commandOpen, setCommandOpen] = useState(false)
  const [suggestionsDismissed, setSuggestionsDismissed] = useState(false)
  const [mentionIndex, setMentionIndex] = useState(0)
  const [mentionOpen, setMentionOpen] = useState(false)
  const [modelMenuOpen, setModelMenuOpen] = useState(false)
  const [permissionMenuOpen, setPermissionMenuOpen] = useState(false)
  const [slashIndex, setSlashIndex] = useState(0)
  const textarea = useRef<HTMLTextAreaElement>(null)

  useEffect(() => {
    document.documentElement.dataset.theme = theme
    void window.desktop.request('setAppearance', { theme }).catch(() => {})
  }, [theme])
  useEffect(() => { savePreference('left-collapsed', String(leftCollapsed)) }, [leftCollapsed])
  useEffect(() => { savePreference('right-collapsed', String(rightCollapsed)) }, [rightCollapsed])
  useEffect(() => {
    const media = window.matchMedia('(prefers-color-scheme: dark)')
    const update = () => { if (!savedPreference('theme')) setTheme(media.matches ? 'dark' : 'light') }
    media.addEventListener('change', update)
    return () => media.removeEventListener('change', update)
  }, [])
  function resizeComposer() {
    const input = textarea.current
    if (input) { input.style.height = 'auto'; input.style.height = `${Math.min(190, Math.max(76, input.scrollHeight))}px` }
  }
  useLayoutEffect(resizeComposer, [draft, items.length === 0, leftCollapsed, rightCollapsed])
  useEffect(() => {
    window.addEventListener('resize', resizeComposer)
    return () => window.removeEventListener('resize', resizeComposer)
  }, [])
  function toggleTheme() { setTheme(value => { const next = value === 'dark' ? 'light' : 'dark'; savePreference('theme', next); return next }) }
  function showSettings() { setSettingsOpen(true); setSettingsTab('general'); if (workspace) loadCapabilities() }
  function suggestPrompt(text: string) { setDraft(text); textarea.current?.focus() }
  function handleAction(action: () => Promise<unknown>) { void action().catch(e => setError(String(e))) }

  async function chooseWorkspace() { const root = await window.desktop.chooseWorkspace(); if (root) await createSession(root) }
  async function newSession() { await createSession(); requestAnimationFrame(() => textarea.current?.focus()) }

  async function updateState(method: string, params: Record<string, unknown>) {
    try { setAgentState(await request<AgentState>(method, params)); setError('') }
    catch (e) { setError(String(e)) }
  }
  const changeModel = (value: string) => updateState('setModel', { model: value })
  const changeEffort = (value: string) => updateState('setEffort', { effort: value })
  const changePermission = (value: string) => updateState('setPermissionMode', { mode: value })
  const changeBudget = (value: AgentState['budget']) => updateState('setBudget', value)
  const changeAcceptance = (path: string) => updateState('setAcceptance', { path })
  async function changeSkill(name: string, active: boolean) {
    if (!workspace) return
    try {
      await request('setSkillActive', { name, active, workspace, sessionId })
      loadCapabilities(); setError('')
    } catch (e) { setError(String(e)) }
  }
  async function changePlugin(name: string, enabled: boolean) {
    if (!workspace) return
    try { setCapabilities(await request<Capabilities>('setPluginEnabled', { name, enabled, workspace })); setMcpDiscovery(null); setError('') }
    catch (e) { setError(String(e)) }
  }
  async function refreshMcp() {
    if (!workspace) return
    try { setMcpDiscovery(await request<McpDiscovery>('refreshMcp', { workspace })); setError('') }
    catch (e) { setError(String(e)) }
  }

  async function executeSlash(text: string) {
    if (busy) { setError('当前回合结束后再执行命令'); return }
    try {
      const result = await request<SlashResult>('runSlash', { text, workspace, sessionId: sessionId })
      if (result.state) setAgentState(result.state)
      if (result.action === 'new') await newSession()
      if (result.action === 'clear') setItems([])
      if (result.action === 'exit') { await request('closeWindow'); return }
      if (result.action === 'resume' && result.sessionId) {
        const selected = sessions.find(session => session.session_id === result.sessionId)
        if (selected) await openSession(selected)
      }
      if (result.action === 'continue') setBusy(true)
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
    if (!text || loading) return
    setDraft(''); setMentionOpen(false)
    if (text.startsWith('/')) { void executeSlash(text); return }
    void sendNow(text)
    requestAnimationFrame(() => textarea.current?.focus())
  }

  async function decide(approvalId: string, granted: boolean) {
    try { await request('resolveApproval', { approvalId, granted }) }
    catch (e) { setError(String(e)) }
  }

  const mention = draft.match(/@([\w./-]*)$/)?.[1]
  const fileSuggestions = !suggestionsDismissed && mentionOpen && mention !== undefined && !draft.startsWith('/') ? files.filter(file => file.toLowerCase().includes(mention.toLowerCase())).slice(0, 7) : []
  function insertFile(file: string) { setDraft(value => value.replace(/@[\w./-]*$/, `@${file} `)); setMentionOpen(false); textarea.current?.focus() }

  let slashOptions: Suggestion[] = []
  if (!suggestionsDismissed && draft.startsWith('/')) {
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
      if (event.defaultPrevented) return
      const modifier = event.ctrlKey || event.metaKey
      if (modifier && event.key.toLowerCase() === 'k') { event.preventDefault(); setSettingsOpen(false); setCommandOpen(value => !value); return }
      if (settingsOpen || commandOpen) return
      if (modifier && event.key.toLowerCase() === 'n') { event.preventDefault(); handleAction(newSession) }
      if (modifier && event.key.toLowerCase() === 'l' && !busy) { event.preventDefault(); setItems([]) }
      if (modifier && ['i', ','].includes(event.key.toLowerCase())) { event.preventDefault(); showSettings() }
      if (modifier && event.key.toLowerCase() === 'b') { event.preventDefault(); if (event.shiftKey) setRightCollapsed(value => !value); else setLeftCollapsed(value => !value) }
      if (modifier && event.key === '.' && busy) { event.preventDefault(); handleAction(() => request('cancelTurn')) }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  })

  const title = sessions.find(session => session.session_id === sessionId)?.title || items.find(item => item.kind === 'user')?.text || '新建任务'
  const supportsEffort = models.find(model => model.id === agentState.model)?.supportsEffort ?? false
  const awaitingApproval = items.some(item => Boolean(item.approvalId))
  const fileIndex = Math.min(mentionIndex, Math.max(0, fileSuggestions.length - 1))
  const stop = () => handleAction(() => request('cancelTurn'))
  const composer = <div className="composer-area">
        <AnimatePresence initial={false}>{error && <motion.div className="error-banner" role="alert" initial={{ opacity: 0, y: 5 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0 }}><CircleAlert size={16} /><span>{error}</span><button className="icon-button" aria-label="关闭提示" onClick={() => setError('')}><X size={14} /></button></motion.div>}</AnimatePresence>
        <AnimatePresence>{busy && <motion.div className="run-status-wrapper" initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0 }}><RunStatus key={activeKey} startedAt={startedAt} approval={awaitingApproval} onStop={stop} /></motion.div>}</AnimatePresence>
        {queue.length > 0 && <div className="queue-list" aria-label="排队消息"><div className="section-label">等待执行 · {queue.length}</div><AnimatePresence initial={false}>{queue.map((text, index) => <motion.div className="queue-item" key={`${index}-${text}`} initial={{ opacity: 0, x: -5 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0, x: 8 }}><ListPlus size={14} /><span title={text}>{text}</span><button className="icon-button" aria-label={`取消排队消息 ${index + 1}`} onClick={() => removeQueued(index)}><X size={13} /></button></motion.div>)}</AnimatePresence></div>}
        <motion.div layoutId="task-composer" transition={{ type: "spring", stiffness: 350, damping: 35 }} className={`composer ${busy ? 'composer-busy' : ''}`}>
          {fileSuggestions.length > 0 && <div className="mention-popover" id="file-suggestions" role="listbox" aria-label="引用本地文件"><div className="section-label">引用本地文件<small>↑↓ 选择 · Tab 插入</small></div>{fileSuggestions.map((file, index) => <button key={file} id={`file-option-${index}`} role="option" aria-selected={index === fileIndex} className={index === fileIndex ? 'slash-option-active' : ''} onMouseDown={event => event.preventDefault()} onClick={() => insertFile(file)}><AtSign size={14} /><span className="truncate">{file}</span></button>)}</div>}
          {slashOptions.length > 0 && <div className="slash-popover" id="slash-suggestions" role="listbox" aria-label="命令建议"><div className="section-label">快捷命令<small>↑↓ 选择 · Enter 执行</small></div>{slashOptions.map((option, index) => <button key={option.value} id={`slash-option-${index}`} role="option" aria-selected={index === optionIndex} className={index === optionIndex ? 'slash-option-active' : ''} onMouseDown={event => event.preventDefault()} onClick={() => chooseSlash(option)}><span className="font-mono text-accent">{option.value}</span><span className="truncate text-xs text-subtle">{option.detail}</span></button>)}</div>}
          <textarea ref={textarea} value={draft} onChange={event => { setDraft(event.target.value); setMentionOpen(true); setSuggestionsDismissed(false); setSlashIndex(0); setMentionIndex(0) }}
            onKeyDown={event => {
              if (event.nativeEvent.isComposing || event.keyCode === 229) return
              if (fileSuggestions.length > 0 && ['ArrowDown', 'ArrowUp'].includes(event.key)) { event.preventDefault(); setMentionIndex(value => (value + (event.key === 'ArrowDown' ? 1 : -1) + fileSuggestions.length) % fileSuggestions.length); return }
              if (fileSuggestions.length > 0 && ['Tab', 'Enter'].includes(event.key) && !event.shiftKey) { event.preventDefault(); insertFile(fileSuggestions[fileIndex]); return }
              if (slashOptions.length > 0 && ['ArrowDown', 'ArrowUp'].includes(event.key)) { event.preventDefault(); setSlashIndex(value => (value + (event.key === 'ArrowDown' ? 1 : -1) + slashOptions.length) % slashOptions.length); return }
              if (slashOptions.length > 0 && ['Tab', 'Enter'].includes(event.key) && !event.shiftKey) { event.preventDefault(); chooseSlash(slashOptions[optionIndex]); return }
              if (event.key === 'Escape') { setModelMenuOpen(false); setPermissionMenuOpen(false); setMentionOpen(false); setSuggestionsDismissed(true); return }
              if (event.key === 'Enter' && !event.shiftKey) { event.preventDefault(); if (workspace || draft.startsWith('/')) submit() }
            }} placeholder={workspace ? '描述任务，或输入 / 查看命令、@ 引用文件…' : '先打开工作区，再描述你想完成的任务…'} rows={2} aria-label="任务输入"
            aria-controls={fileSuggestions.length ? 'file-suggestions' : slashOptions.length ? 'slash-suggestions' : undefined} aria-activedescendant={fileSuggestions.length ? `file-option-${fileIndex}` : slashOptions.length ? `slash-option-${optionIndex}` : undefined} />
          <div className="composer-toolbar">
            <button className="composer-icon" title="引用文件" aria-label="引用文件" disabled={!workspace} onClick={() => { setDraft(value => value + (value && !value.endsWith(' ') ? ' @' : '@')); setMentionOpen(true); setSuggestionsDismissed(false); setMentionIndex(0); textarea.current?.focus() }}><Plus size={18} /></button>
            <span className="toolbar-divider" />
            <SelectMenu label="切换模型" value={agentState.model} icon={<Cpu size={14} />} open={modelMenuOpen} onOpen={open => { setModelMenuOpen(open); setPermissionMenuOpen(false) }} onSelect={changeModel}
              options={models.map(option => ({ value: option.id, label: option.id === 'fake' ? '离线演示' : option.id, detail: option.available ? option.provider : '尚未配置', disabled: !option.available && option.id !== agentState.model }))}
              secondary={{ label: '思考强度', value: agentState.effort, onSelect: changeEffort, hint: supportsEffort ? undefined : '当前模型不支持',
                options: [['off', '关闭'], ['low', '低'], ['medium', '中'], ['high', '高'], ['xhigh', '很高'], ['max', '最高']].map(([value, label]) => ({ value, label, disabled: !supportsEffort })) }} />
            <SelectMenu label="权限模式" value={agentState.permissionMode} icon={<Shield size={13} />} open={permissionMenuOpen} onOpen={open => { setPermissionMenuOpen(open); setModelMenuOpen(false) }} onSelect={changePermission}
              options={[{ value: 'default', label: '逐项确认', detail: '写入与命令执行前请求批准' }, { value: 'accept_edits', label: '自动编辑', detail: '文件编辑自动批准，命令仍需确认' }, { value: 'bypass', label: '全部允许', detail: '自动批准所有工具操作' }]} />
            <div className="composer-send"><ContextMeter used={agentState.contextTokens} windowSize={agentState.contextWindow} breakdown={agentState.contextBreakdown || { system: 0, tools: 0, messages: 0 }}
              onOpen={() => { void request<AgentState>('getState').then(setAgentState).catch(e => setError(String(e))) }} />
              <button className={`send-button ${busy && !draft.trim() ? 'send-stop' : ''}`} onClick={busy && !draft.trim() ? stop : submit} disabled={loading || (!busy && (!draft.trim() || (!workspace && !draft.startsWith('/'))))} aria-label={busy ? draft.trim() ? '加入队列' : '停止任务' : '发送任务'} title={busy ? draft.trim() ? '加入队列' : '停止任务 · Ctrl .' : '发送任务'}>{busy ? draft.trim() ? <ListPlus size={18} /> : <Square size={13} fill="currentColor" /> : <ArrowUp size={19} />}</button>
            </div>
          </div>
        </motion.div>
      </div>
  return <MotionConfig reducedMotion="user"><div className="window-titlebar"><span>MiniCode</span><span className="window-titlebar-project">{workspace ? workspace.split(/[\\/]/).pop() : 'Desktop'}</span></div>
  <div className={`app-shell ${leftCollapsed ? 'left-collapsed' : ''} ${rightCollapsed ? 'right-collapsed' : ''}`}>
    <Sidebar workspace={workspace} sessions={sessions} activeSession={sessionId} runningSessions={Object.values(views).filter(view => view.busy && view.sessionId).map(view => ({ id: view.sessionId!, approval: view.items.some(item => Boolean(item.approvalId)) }))} unreadSessions={Object.values(views).filter(view => view.unread && view.sessionId).map(view => view.sessionId!)} theme={theme} onTheme={toggleTheme}
      collapsed={leftCollapsed} onToggle={() => setLeftCollapsed(value => !value)} onSettings={showSettings}
      onChoose={() => handleAction(chooseWorkspace)} onNew={() => handleAction(newSession)} onWorkspace={root => handleAction(() => createSession(root))} onSession={session => handleAction(() => openSession(session))} />
    <main className={`main-panel ${items.length ? "has-conversation" : "is-empty"}`}>
      <header className="main-header">
        <div className="main-heading"><span className="section-label">{sessionId ? '任务' : '工作空间'}</span><h2 title={title}>{title}</h2></div>
        <div className="header-actions"><span className={`connection-status ${busy ? 'status-running' : ''} ${awaitingApproval ? 'status-waiting' : ''}`}><i />{awaitingApproval ? '等待审批' : loading ? '载入中' : busy ? '运行中' : workspace ? '就绪' : '未选择工作区'}</span>
          <button className="icon-button" title="快捷操作 · Ctrl K" aria-label="打开快捷操作" onClick={() => setCommandOpen(true)}><Search size={16} /></button>
          <button className="icon-button" title="新建任务 · Ctrl N" aria-label="新建任务" onClick={() => handleAction(newSession)}><SquarePen size={17} /></button>
        </div>
      </header>
      <SessionFeed key={activeKey} sessionId={sessionId} clientKey={activeKey} items={items} busy={busy} workspace={workspace} onChoose={() => handleAction(chooseWorkspace)} onPrompt={suggestPrompt} onDecide={decide}>{items.length === 0 ? composer : null}</SessionFeed>
      {items.length > 0 && composer}
    </main>
    <WorkPanel workspace={workspace} changes={changes} files={files} items={items} tasks={tasks} state={agentState} trace={trace.filter(Boolean) as NonNullable<DesktopEvent['item']>[]} refresh={refresh}
      collapsed={rightCollapsed} onToggle={() => setRightCollapsed(value => !value)} />
  </div>
  <AnimatePresence mode="wait">
    {settingsOpen && <SettingsPanel key="settings" tab={settingsTab} setTab={setSettingsTab} onClose={() => setSettingsOpen(false)}
      models={models} state={agentState} capabilities={capabilities} mcpDiscovery={mcpDiscovery} trace={trace.filter(Boolean) as NonNullable<DesktopEvent['item']>[]} error={error}
      onModel={changeModel} onEffort={changeEffort} onPermission={changePermission} onBudget={changeBudget} onAcceptance={changeAcceptance}
      onChooseAcceptance={() => window.desktop.chooseAcceptanceFile()} onSkill={changeSkill}
      onPlugin={changePlugin} onRefreshMcp={refreshMcp} onLockPlugins={async () => { if (workspace) { try { await request('lockPlugins', { workspace }); loadCapabilities(); setError('') } catch (e) { setError(String(e)) } } }}
      onUseAgent={kind => { setDraft(value => `${value ? `${value.trimEnd()}\n` : ''}请使用 ${kind} 子助手调查并报告证据。`); setSettingsOpen(false); textarea.current?.focus() }} />}
    {commandOpen && <CommandPalette key="commands" sessions={sessions} onClose={() => setCommandOpen(false)} onNew={() => handleAction(newSession)} onChoose={() => handleAction(chooseWorkspace)} onSettings={showSettings} onLeft={() => setLeftCollapsed(value => !value)} onRight={() => setRightCollapsed(value => !value)} onTheme={toggleTheme} onSession={session => handleAction(() => openSession(session))} />}
  </AnimatePresence></MotionConfig>
}
