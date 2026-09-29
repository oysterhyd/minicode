import { useEffect, useRef, useState } from 'react'
import { AnimatePresence, LayoutGroup, MotionConfig } from 'motion/react'
import { Sidebar } from './components/Sidebar'
import { TitleBar } from './components/TitleBar'
import { SessionFeed } from './components/feed/SessionFeed'
import { Welcome } from './components/feed/Welcome'
import { Composer } from './components/composer/Composer'
import { WorkPanel } from './components/work/WorkPanel'
import { SettingsPanel, type SettingsTab } from './components/settings/SettingsPanel'
import { CommandPalette } from './components/CommandPalette'
import { ShortcutsDialog } from './components/Shortcuts'
import { ConfirmDialog } from './components/ui/Modal'
import { ErrorBoundary } from './components/ui/ErrorBoundary'
import { useToast } from './components/ui/Toast'
import { Splitter } from './components/ui/Splitter'
import type { AgentState, Capabilities, McpDiscovery, Session, SlashResult } from './types'
import { useConversations, type Signal } from './hooks/useConversations'
import { rememberPrompt, usePreferences, useSystemDark } from './lib/prefs'

const clean = (error: unknown) => String(error).replace(/^Error: /, '')

export default function App() {
  const [prefs, setPref] = usePreferences()
  const systemDark = useSystemDark()
  const theme = prefs.theme === 'system' ? systemDark ? 'dark' : 'light' : prefs.theme
  const toast = useToast()
  const signalRef = useRef<(signal: Signal) => void>(() => {})
  const c = useConversations(signal => signalRef.current(signal))
  const { workspace, sessions, sessionId, items, busy, loading, error, activeKey, views, agentState } = c
  const [discoveries, setDiscoveries] = useState<Record<string, McpDiscovery | null>>({})
  const [settings, setSettings] = useState<SettingsTab | null>(null)
  const [dialog, setDialog] = useState<'commands' | 'shortcuts' | null>(null)
  const [pendingDelete, setPendingDelete] = useState<Session | null>(null)
  const textarea = useRef<HTMLTextAreaElement>(null)

  useEffect(() => {
    document.documentElement.dataset.theme = theme
    void window.desktop.request('setAppearance', { theme }).catch(() => {})
  }, [theme])
  useEffect(() => { document.documentElement.style.setProperty('--ui-scale', String(prefs.fontScale)) }, [prefs.fontScale])

  function focusComposer() { requestAnimationFrame(() => textarea.current?.focus()) }
  function act(action: () => Promise<unknown>, failure = '操作失败') { void action().catch(e => toast({ tone: 'error', title: failure, detail: clean(e) })) }
  function notify(title: string, body: string, targetSession: string | null) {
    if (prefs.notifications && !document.hasFocus()) void window.desktop.request('notify', { title, body, sessionId: targetSession }).catch(() => {})
  }
  signalRef.current = signal => {
    if (signal.type === 'notification_click') {
      const session = sessions.find(item => item.session_id === signal.sessionId)
      if (session) act(() => c.openSession(session))
      return
    }
    const background = signal.key !== activeKey
    const show = () => c.activateKey(signal.key)
    if (signal.type === 'approval') {
      notify('MiniCode 需要你的批准', `${signal.title} · ${signal.tool}`, signal.sessionId)
      if (background) toast({ tone: 'info', title: '后台任务等待审批', detail: signal.title, action: { label: '查看', run: show } })
      return
    }
    if (signal.reason === 'cancelled') return
    notify(signal.ok ? '任务已完成' : '任务已结束', signal.title, signal.sessionId)
    if (background) toast({ tone: signal.ok ? 'success' : 'error', title: signal.ok ? '后台任务已完成' : '后台任务未完成', detail: signal.title, action: { label: '查看', run: show } })
  }

  const toggleTheme = () => setPref('theme', theme === 'dark' ? 'light' : 'dark')
  const toggleLeft = () => setPref('leftCollapsed', value => !value)
  const toggleRight = () => setPref('rightCollapsed', value => !value)
  function showSettings(tab: SettingsTab = 'general') { setDialog(null); setSettings(tab); if (workspace) void c.loadCapabilities() }
  async function chooseWorkspace() { const root = await window.desktop.chooseWorkspace(); if (root) { await c.newSession(root); focusComposer() } }
  async function newSession() { await c.newSession(); focusComposer() }
  function insertMention(path: string) { c.setDraft(value => `${value}${value && !value.endsWith(' ') ? ' ' : ''}@${path} `); focusComposer() }

  async function updateState(method: string, params: Record<string, unknown>) {
    try { c.setAgentState(await c.request<AgentState>(method, params)); c.setError('') }
    catch (e) { toast({ tone: 'error', title: '设置未生效', detail: clean(e) }) }
  }
  const setMcp = (value: McpDiscovery | null) => setDiscoveries(previous => ({ ...previous, [activeKey]: value }))

  async function executeSlash(text: string) {
    if (busy && !/^\/(help|\?)\b/.test(text)) { toast({ tone: 'info', title: '当前回合结束后再执行命令' }); return }
    try {
      const result = await c.request<SlashResult>('runSlash', { text, workspace, sessionId })
      if (result.state) c.setAgentState(result.state)
      if (result.action === 'new') await newSession()
      if (result.action === 'clear') c.setItems([])
      if (result.action === 'exit') { await c.request('closeWindow'); return }
      if (result.action === 'resume' && result.sessionId) {
        const selected = sessions.find(session => session.session_id === result.sessionId)
        if (selected) await c.openSession(selected)
      }
      if (result.action === 'continue') c.setBusy(true)
      if (result.action === 'skills') showSettings('skills')
      if (['model', 'effort', 'permissions', 'sessions'].includes(result.action || '')) { c.setDraft(result.action === 'sessions' ? '/resume ' : `/${result.action} `); focusComposer() }
      if (result.message && result.action !== 'new' && result.action !== 'clear') c.setItems(previous => [...previous, { id: `system-${Date.now()}`, kind: 'notice', text: result.message }])
      c.setError('')
    } catch (e) { c.setError(clean(e)) }
  }
  function submit(text: string) { rememberPrompt(text); c.setDraft(''); void c.sendNow(text); focusComposer() }
  async function decide(approvalId: string, granted: boolean, remember = false) {
    try {
      await c.request('resolveApproval', { approvalId, granted, ...(remember && granted ? { remember: 'session' } : {}) })
      if (remember) void c.request<AgentState>('getState').then(c.setAgentState).catch(() => {})
      focusComposer()
    } catch (e) { toast({ tone: 'error', title: '审批未送达', detail: clean(e) }) }
  }
  const stop = () => act(() => c.request('cancelTurn'), '无法停止任务')
  function switchSession(offset: number) {
    const index = sessions.findIndex(session => session.session_id === sessionId)
    const next = sessions[(index + offset + sessions.length) % sessions.length]
    if (next) act(() => c.openSession(next))
  }
  const keyHandler = useRef<(event: KeyboardEvent) => void>(() => {})
  keyHandler.current = event => {
    if (event.defaultPrevented) return
    const modifier = event.ctrlKey || event.metaKey
    if (!modifier) return
    const key = event.key.toLowerCase()
    if (key === 'k') { event.preventDefault(); setSettings(null); setDialog(value => value === 'commands' ? null : 'commands'); return }
    if (settings || dialog || pendingDelete) return
    if (key === 'n') { event.preventDefault(); act(newSession) }
    else if (key === 'l' && event.shiftKey) { event.preventDefault(); toggleTheme() }
    else if (key === 'l' && !busy) { event.preventDefault(); c.setItems([]) }
    else if (key === ',' || key === 'i') { event.preventDefault(); showSettings() }
    else if (key === '/' || key === '?') { event.preventDefault(); setDialog('shortcuts') }
    else if (key === 'b') { event.preventDefault(); if (event.shiftKey) toggleRight(); else toggleLeft() }
    else if (key === 'f' && event.shiftKey) { event.preventDefault(); setPref('leftCollapsed', false); setTimeout(() => window.dispatchEvent(new Event('minicode:focus-search')), 30) }
    else if (key === '.' && busy) { event.preventDefault(); stop() }
    else if ((event.code === 'BracketLeft' || event.code === 'BracketRight') && event.shiftKey) { event.preventDefault(); switchSession(event.code === 'BracketLeft' ? -1 : 1) }
    else if (/^[1-4]$/.test(key) && !event.shiftKey) {
      event.preventDefault(); setPref('rightCollapsed', false)
      window.dispatchEvent(new CustomEvent('minicode:work-tab', { detail: ['changes', 'files', 'terminal', 'tasks'][Number(key) - 1] }))
    }
  }
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => keyHandler.current(event)
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

  const title = sessions.find(session => session.session_id === sessionId)?.title || items.find(item => item.kind === 'user')?.text?.split('\n')[0] || '新建任务'
  const awaitingApproval = items.some(item => Boolean(item.approvalId))
  const running: Record<string, { approval: boolean }> = {}
  for (const view of Object.values(views)) if (view.busy && view.sessionId) running[view.sessionId] = { approval: view.items.some(item => Boolean(item.approvalId)) }
  const unread = Object.values(views).filter(view => view.unread && view.sessionId).map(view => view.sessionId!)
  const home = items.length === 0
  const composer = <Composer textarea={textarea} draft={c.draft} setDraft={c.setDraft} workspace={workspace} busy={busy} loading={loading}
    awaitingApproval={awaitingApproval} startedAt={c.startedAt} error={error} onDismissError={() => c.setError('')} queue={c.queue} onRemoveQueued={c.removeQueued}
    state={agentState} models={c.models} commands={c.commands} sessions={sessions} capabilities={c.capabilities} files={c.files} sendKey={prefs.sendKey} home={home}
    onSubmit={submit} onStop={stop} onSlash={text => void executeSlash(text)}
    onModel={value => void updateState('setModel', { model: value })} onEffort={value => void updateState('setEffort', { effort: value })}
    onPermission={value => void updateState('setPermissionMode', { mode: value })}
    onRefreshState={() => { void c.request<AgentState>('getState').then(c.setAgentState).catch(() => {}) }} onCompact={() => void executeSlash('/compact')} />

  return <MotionConfig reducedMotion="user"><LayoutGroup>
    <div className="app-frame">
      <TitleBar workspace={workspace} title={title} status={awaitingApproval ? 'waiting' : loading ? 'loading' : busy ? 'running' : workspace ? 'idle' : 'none'}
        leftCollapsed={prefs.leftCollapsed} rightCollapsed={prefs.rightCollapsed} onLeft={toggleLeft} onRight={toggleRight} onSearch={() => setDialog('commands')} onNew={() => act(newSession)} />
      <div className={`app-shell ${prefs.leftCollapsed ? 'left-collapsed' : ''} ${prefs.rightCollapsed ? 'right-collapsed' : ''}`}
        style={{ '--sidebar-width': `${prefs.sidebarWidth}px`, '--work-width': `${prefs.workWidth}px` } as React.CSSProperties}>
        <Sidebar workspace={workspace} sessions={sessions} activeSession={sessionId} running={running} unread={unread} theme={prefs.theme}
          collapsed={prefs.leftCollapsed} onToggle={toggleLeft} onTheme={toggleTheme} onSettings={() => showSettings()}
          onChoose={() => act(chooseWorkspace)} onNew={() => act(newSession)} onWorkspace={root => act(() => c.newSession(root))} onSession={session => act(() => c.openSession(session), '无法打开任务')}
          onRename={(session, name) => act(async () => { await c.renameSession(session.session_id, name); toast({ tone: 'success', title: '已重命名' }) }, '重命名失败')}
          onPin={session => act(() => c.pinSession(session.session_id, !session.pinned))} onDelete={setPendingDelete} />
        {!prefs.leftCollapsed && <Splitter label="调整侧栏宽度" value={prefs.sidebarWidth} min={220} max={400} onChange={value => setPref('sidebarWidth', value)} />}
        <main className={`main-panel ${home ? 'is-empty' : 'has-conversation'}`}>
          <ErrorBoundary label="对话区域出现问题">
            {home ? <Welcome workspace={workspace} sessions={sessions} onChoose={() => act(chooseWorkspace)} onPrompt={text => { c.setDraft(text); focusComposer() }}
              onSession={session => act(() => c.openSession(session))}>{composer}</Welcome>
              : <><SessionFeed key={activeKey} sessionId={sessionId} clientKey={activeKey} items={items} busy={busy} expandTools={prefs.expandTools} onDecide={decide}
                onEdit={text => { c.setDraft(text); focusComposer() }} onRetry={text => submit(text)} />{composer}</>}
          </ErrorBoundary>
        </main>
        {!prefs.rightCollapsed && <Splitter label="调整工作台宽度" value={prefs.workWidth} min={300} max={720} invert onChange={value => setPref('workWidth', value)} />}
        <ErrorBoundary label="工作台出现问题" compact>
          <WorkPanel workspace={workspace} changes={c.changes} files={c.files} items={items} tasks={c.tasks} state={agentState} refresh={c.refresh}
            collapsed={prefs.rightCollapsed} onToggle={toggleRight} onMention={insertMention} />
        </ErrorBoundary>
      </div>
    </div>
    <AnimatePresence>
      {settings && <SettingsPanel key="settings" tab={settings} setTab={setSettings} onClose={() => setSettings(null)} prefs={prefs} setPref={setPref}
        models={c.models} state={agentState} capabilities={c.capabilities} mcpDiscovery={discoveries[activeKey] || null} trace={c.trace} error={error} busy={busy}
        onModel={value => void updateState('setModel', { model: value })} onEffort={value => void updateState('setEffort', { effort: value })}
        onPermission={value => void updateState('setPermissionMode', { mode: value })} onBudget={value => void updateState('setBudget', value)}
        onAcceptance={path => void updateState('setAcceptance', { path })} onChooseAcceptance={() => window.desktop.chooseAcceptanceFile()}
        onClearAlwaysAllow={() => void updateState('clearAlwaysAllow', {})}
        onSkill={(name, active) => act(async () => { await c.request('setSkillActive', { name, active, workspace, sessionId }); await c.loadCapabilities() }, '技能切换失败')}
        onPlugin={(name, enabled) => act(async () => { c.setCapabilities(await c.request<Capabilities>('setPluginEnabled', { name, enabled, workspace })); setMcp(null) }, '插件切换失败')}
        onRefreshMcp={async () => { try { setMcp(await c.request<McpDiscovery>('refreshMcp', { workspace })) } catch (e) { toast({ tone: 'error', title: 'MCP 发现失败', detail: clean(e) }) } }}
        onLockPlugins={() => act(async () => { await c.request('lockPlugins', { workspace }); await c.loadCapabilities(); toast({ tone: 'success', title: '插件锁定已更新' }) }, '更新锁定失败')}
        onUseAgent={kind => { c.setDraft(value => `${value ? `${value.trimEnd()}\n` : ''}请使用 ${kind} 子助手调查并报告证据。`); setSettings(null); focusComposer() }} />}
      {dialog === 'commands' && <CommandPalette key="commands" sessions={sessions} files={c.files} onClose={() => setDialog(null)} onNew={() => act(newSession)} onChoose={() => act(chooseWorkspace)}
        onSettings={() => showSettings()} onShortcuts={() => setDialog('shortcuts')} onLeft={toggleLeft} onRight={toggleRight} onTheme={toggleTheme}
        onSession={session => act(() => c.openSession(session))} onFile={insertMention} />}
      {dialog === 'shortcuts' && <ShortcutsDialog key="shortcuts" onClose={() => setDialog(null)} />}
      {pendingDelete && <ConfirmDialog key="delete" title="删除任务？" confirmLabel="删除" danger onClose={() => setPendingDelete(null)}
        detail={<>“{pendingDelete.title}”的对话记录、运行事件和归档输出将被永久删除，工作区中的文件不受影响。</>}
        onConfirm={() => act(async () => { await c.deleteSession(pendingDelete.session_id); toast({ tone: 'success', title: '任务已删除' }) }, '删除失败')} />}
    </AnimatePresence>
  </LayoutGroup></MotionConfig>
}
