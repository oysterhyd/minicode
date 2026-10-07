import { useApp, useStdout } from 'ink'
import { Box, Text, type DOMElement } from 'ink'
import { useEffect, useMemo, useRef, useState } from 'react'
import type { LaunchOptions } from './args.js'
import type { BridgeApi } from './bridge.js'
import {
  applySuggestion,
  isSubmenu,
  slashSuggestions,
  SUBMENU_TITLES,
  type Suggestion,
} from './slash.js'
import {
  clamp,
  deleteBackward,
  deleteForward,
  insertAt,
  lineEnd,
  lineStart,
  moveVertical,
  nextBoundary,
  nextWord,
  prevBoundary,
  prevWord,
  promptRows,
} from './caret.js'
import { applyDesktopEvent, emptyFeed, pendingApproval, type FeedState } from './feed.js'
import { AssistantMarkdown } from './components/AssistantMarkdown.js'
import { ApprovalBlock } from './components/ApprovalBlock.js'
import { Banner, type BannerProps } from './components/Banner.js'
import { Prompt, SlashMenu } from './components/Prompt.js'
import { Notice, Spinner, StatusLine } from './components/StatusLine.js'
import { ThoughtLine, ToolLine } from './components/ToolLine.js'
import { UserBubble } from './components/UserBubble.js'
import { TranscriptViewport, type TranscriptScroll } from './components/TranscriptViewport.js'
import { lastExpandableTool, layoutTranscript, type LayoutItem } from './transcript.js'
import type {
  AgentState,
  Capabilities,
  Command,
  DesktopEvent,
  FeedItem,
  InitializeResult,
  ModelInfo,
  SessionDetail,
  SessionSummary,
  SkillInfo,
  SlashResult,
} from './types.js'
import { emptyState, historyItems } from './types.js'
import { terminalText } from './format.js'
import { ACCENT, clearTranscript, useTerminalInput, useTerminalSize } from './terminal.js'

const LIVE_SLASH = /^\/(help|\?|model|effort|permissions|skill)(?:\s|$)/i

export type AppProps = {
  client: BridgeApi
  options: LaunchOptions
  version?: string
}

type FrozenItem = LayoutItem | ({ kind: 'banner' } & BannerProps)

function cleanError(error: unknown): string {
  return String(error instanceof Error ? error.message : error).replace(/^Error: /, '')
}

function lastUserIndex(items: FeedItem[]): number {
  return items.reduce((found, item, index) => item.kind === 'user' ? index : found, -1)
}

function FeedRow({ item, now, expanded, detailOnly }: { item: FrozenItem; now: number; expanded: Set<string>; detailOnly?: boolean }) {
  if (item.kind === 'banner') {
    return <Banner version={item.version} workspace={item.workspace} model={item.model} permissionMode={item.permissionMode} />
  }
  if (item.kind === 'thought') return <ThoughtLine item={item} now={now} />
  if (item.kind === 'user') return <UserBubble text={item.text || ''} />
  if (item.kind === 'assistant') return <AssistantMarkdown text={item.text || ''} lead={item.lead !== false} />
  if (item.kind === 'tool') return <ToolLine item={item} now={now} expanded={expanded.has(item.id)} detailOnly={detailOnly} />
  if (item.kind === 'approval') return <ApprovalBlock item={item} />
  return <Notice text={item.text || ''} tone={item.tone} />
}

export function App({ client, options, version = '1.1.0' }: AppProps) {
  const { exit } = useApp()
  const [feed, setFeed] = useState<FeedState>(emptyFeed)
  const [agent, setAgent] = useState<AgentState>(emptyState)
  const [commands, setCommands] = useState<Command[]>([])
  const [models, setModels] = useState<ModelInfo[]>([])
  const [sessions, setSessions] = useState<SessionSummary[]>([])
  const [skills, setSkills] = useState<SkillInfo[]>([])
  const [draft, setDraft] = useState('')
  const [caret, setCaret] = useState(0)
  const [queued, setQueued] = useState<string[]>([])
  const [slashIndex, setSlashIndex] = useState(0)
  const [ready, setReady] = useState(false)
  const [now, setNow] = useState(Date.now())
  const [outputStart, setOutputStart] = useState(0)
  const [banner, setBanner] = useState<BannerProps | null>(null)
  const [expanded, setExpanded] = useState<Set<string>>(new Set())
  const [scrolled, setScrolled] = useState(false)
  const transcriptRef = useRef<TranscriptScroll>(null)
  const returnButtonRef = useRef<DOMElement>(null)
  const feedRef = useRef(feed)
  const busyRef = useRef(false)
  const queuedRef = useRef<string[]>([])
  const sessionRef = useRef<string | null>(null)
  const draftRef = useRef('')
  const caretRef = useRef(0)
  const agentRef = useRef(agent)
  const workspaceRef = useRef(options.workspace)
  const operationRef = useRef(false)
  const activeRef = useRef(true)
  const readyRef = useRef(false)
  const stateRevision = useRef(0)
  const ctrlCRef = useRef(0)
  const approvalRef = useRef<string | null>(null)
  const drainAllowedRef = useRef(false)
  const cancelPendingRef = useRef(false)
  const connectionFailedRef = useRef(false)
  const noticeRef = useRef(0)
  const slashIndexRef = useRef(0)
  const slashNavigatedRef = useRef(false)
  const { stdout } = useStdout()
  const terminal = useTerminalSize()
  const sendRef = useRef<(text: string) => Promise<void>>(async () => {})

  // Event handlers can run several times before React commits. These refs are
  // the authoritative state; update them when accepting a transition.
  function updateFeed(change: FeedState | ((current: FeedState) => FeedState)) {
    if (!activeRef.current) return
    const next = typeof change === 'function' ? change(feedRef.current) : change
    feedRef.current = next
    busyRef.current = next.busy
    sessionRef.current = next.sessionId
    setFeed(next)
  }
  function updateAgent(state: AgentState) {
    if (!activeRef.current) return
    stateRevision.current += 1
    agentRef.current = state
    setAgent(state)
  }
  function updateQueue(next: string[]) {
    queuedRef.current = next
    if (activeRef.current) setQueued(next)
  }
  function drainQueue() {
    if (!activeRef.current || !drainAllowedRef.current || busyRef.current || operationRef.current) return
    const [next, ...rest] = queuedRef.current
    if (!next) return
    updateQueue(rest)
    void sendRef.current(next)
  }
  async function refreshState() {
    const revision = ++stateRevision.current
    const sessionId = sessionRef.current
    try {
      const state = await client.request<AgentState>('getState', { sessionId, workspace: workspaceRef.current })
      if (activeRef.current && revision === stateRevision.current && sessionId === sessionRef.current) updateAgent(state)
    } catch { /* a run error is already shown by the event handler */ }
  }
  async function refreshSessions() {
    try {
      const rows = await client.request<SessionSummary[]>('listSessions')
      if (activeRef.current) setSessions(rows)
    } catch { /* suggestions can continue using the last successful list */ }
  }

  useEffect(() => {
    if (!feed.busy) return
    const timer = setInterval(() => setNow(Date.now()), 400)
    return () => clearInterval(timer)
  }, [feed.busy])

  useEffect(() => {
    let active = true
    activeRef.current = true
    const unsubscribe = client.onEvent((event: DesktopEvent) => {
      if (!active) return
      if (event.sessionId && sessionRef.current && event.sessionId !== sessionRef.current) return
      updateFeed(current => applyDesktopEvent(current, event))
      if (event.event === 'bridge_error' && event.fatal) {
        connectionFailedRef.current = true
        readyRef.current = false
        setReady(false)
      }
      if (event.event === 'agent_event' && event.item?.type === 'approval_decision') approvalRef.current = null
      if (event.event === 'run_done' || event.event === 'run_error') {
        cancelPendingRef.current = false
        approvalRef.current = null
        drainAllowedRef.current = event.event === 'run_done' && event.result?.exit_reason === 'completed'
        void refreshState()
        void refreshSessions()
        queueMicrotask(drainQueue)
      }
      if (event.event === 'agent_event' && event.item && ['assistant_message', 'subagent_result', 'context_compacted'].includes(event.item.type)) {
        void refreshState()
      }
    })
    void (async () => {
      try {
        const initial = await client.request<InitializeResult>('initialize', { workspace: workspaceRef.current })
        if (!active) return
        setCommands(initial.commands)
        setModels(initial.models || [])
        setSessions(initial.sessions || [])
        let state = initial.state
        if ((options.provider && options.provider !== 'auto') || options.model || options.script) {
          state = await client.request<AgentState>('setTuiProvider', {
            provider: options.provider || 'auto', model: options.model,
            workspace: workspaceRef.current,
          })
        }
        if (options.yes) {
          state = await client.request<AgentState>('setPermissionMode', { mode: 'bypass', workspace: workspaceRef.current })
        }
        if (options.maxRounds !== undefined || options.maxTokens !== undefined || options.maxSeconds !== undefined) {
          state = await client.request<AgentState>('setBudget', {
            max_rounds: options.maxRounds ?? state.budget.max_rounds,
            max_total_tokens: options.maxTokens ?? state.budget.max_total_tokens,
            max_seconds: options.maxSeconds ?? state.budget.max_seconds,
            workspace: workspaceRef.current,
          })
        }
        if (options.acceptance) {
          state = await client.request<AgentState>('setAcceptance', { path: options.acceptance, workspace: workspaceRef.current })
        }
        if (!active) return
        updateAgent(state)
        updateFeed(current => ({ ...current, sessionId: state.sessionId }))
        setBanner({
          version,
          workspace: workspaceRef.current,
          model: state.model,
          permissionMode: state.permissionMode,
        })
        try {
          const capabilities = await client.request<Capabilities>('getCapabilities', { workspace: workspaceRef.current })
          if (active) setSkills(capabilities.skills || [])
        } catch {
          if (active) setSkills([])
        }
        if (active && !connectionFailedRef.current) { readyRef.current = true; setReady(true) }
      } catch (error) {
        if (active) updateFeed(current => ({ ...current, error: cleanError(error) }))
      }
    })()
    return () => { active = false; activeRef.current = false; readyRef.current = false; unsubscribe() }
  }, [client, options, version])

  const matches = useMemo(
    () => slashSuggestions(draft, { commands, models, sessions, skills }),
    [commands, draft, models, sessions, skills],
  )

  function addNotice(text: string, tone: FeedItem['tone'] = 'info') {
    updateFeed(current => ({
      ...current,
      items: [...current.items, { id: `notice-${++noticeRef.current}`, kind: 'notice', text, tone }],
    }))
  }

  function resetScreen(items: FeedItem[] = []) {
    transcriptRef.current?.returnToLatest()
    clearTranscript(stdout)
    setExpanded(new Set())
    setBanner({
      version,
      workspace: workspaceRef.current,
      model: agentRef.current.model,
      permissionMode: agentRef.current.permissionMode,
    })
    updateFeed(current => ({ ...current, items, error: '', startedAt: null }))
  }

  async function runSlash(text: string) {
    if (operationRef.current) { addNotice('上一条命令仍在处理，请稍后重试'); return }
    if (busyRef.current && !LIVE_SLASH.test(text)) {
      addNotice('当前回合结束后再执行命令')
      return
    }
    operationRef.current = true
    stateRevision.current += 1
    if (/^\/(new|resume)(?:\s|$)/i.test(text)) updateQueue([])
    let succeeded = false
    const continuing = /^\/continue(?:\s|$)/i.test(text)
    if (continuing) {
      setOutputStart(agentRef.current.usage?.output_tokens || 0)
      drainAllowedRef.current = false
      updateFeed(current => ({ ...current, busy: true, startedAt: Date.now(), error: '' }))
    }
    try {
      const result = await client.request<SlashResult>('runSlash', {
        text,
        workspace: workspaceRef.current,
        sessionId: sessionRef.current,
      })
      if (!activeRef.current) return
      if (result.state) updateAgent(result.state)
      if (result.action === 'exit') {
        exit()
        return
      }
      if (result.action === 'clear') {
        resetScreen([])
      }
      if (result.action === 'new') {
        drainAllowedRef.current = false
        updateFeed({ ...emptyFeed(), sessionId: null })
        const state = await client.request<AgentState>('getState', { workspace: workspaceRef.current })
        updateAgent(state)
        resetScreen([])
        void refreshSessions()
      }
      if (result.action === 'resume' && result.sessionId) {
        const detail = await client.request<SessionDetail>('getSession', { sessionId: result.sessionId, workspace: workspaceRef.current })
        const state = await client.request<AgentState>('selectSession', { sessionId: result.sessionId, workspace: workspaceRef.current })
        if (!activeRef.current) return
        workspaceRef.current = detail.summary.workspace
        updateAgent(state)
        drainAllowedRef.current = false
        setExpanded(new Set())
        transcriptRef.current?.returnToLatest()
        clearTranscript(stdout)
        setBanner({
          version,
          workspace: workspaceRef.current,
          model: state.model,
          permissionMode: state.permissionMode,
        })
        updateFeed({ items: historyItems(detail), busy: false, error: '', startedAt: null, sessionId: result.sessionId })
        try {
          const capabilities = await client.request<Capabilities>('getCapabilities', { workspace: workspaceRef.current })
          if (activeRef.current) setSkills(capabilities.skills || [])
        } catch { if (activeRef.current) setSkills([]) }
      }
      if (result.action === 'model') addNotice('用法：/model <名称>')
      if (result.action === 'effort') addNotice('用法：/effort off|low|medium|high|xhigh|max')
      if (result.action === 'permissions') addNotice('用法：/permissions default|accept_edits|bypass')
      if (result.action === 'skills') addNotice(result.message || '用法：/skill <名称> 或 /skill off <名称>')
      if (result.action === 'sessions') addNotice(result.message || '暂无会话。')
      if (result.message && !['model', 'effort', 'permissions', 'skills', 'sessions'].includes(result.action || '')) {
        addNotice(result.message)
      }
      succeeded = true
    } catch (error) {
      if (continuing) updateFeed(current => ({ ...current, busy: false, startedAt: null }))
      addNotice(cleanError(error), 'error')
    } finally {
      operationRef.current = false
      if (succeeded && !continuing && !busyRef.current) drainAllowedRef.current = true
      if (continuing && !busyRef.current) void refreshState()
      drainQueue()
    }
  }

  async function send(text: string) {
    const trimmed = text.trim()
    if (!trimmed) return
    if (trimmed.startsWith('/')) {
      await runSlash(trimmed)
      return
    }
    if (busyRef.current || operationRef.current) {
      updateQueue([...queuedRef.current, trimmed])
      return
    }
    operationRef.current = true
    drainAllowedRef.current = false
    stateRevision.current += 1
    const started = Date.now()
    setOutputStart(agentRef.current.usage?.output_tokens || 0)
    setExpanded(new Set())
    updateFeed(current => ({
      ...current,
      error: '',
      busy: true,
      startedAt: started,
      items: [...current.items, { id: `user-${started}`, kind: 'user', text: trimmed, startedAt: started }],
    }))
    try {
      const result = await client.request<{ sessionId: string }>('sendPrompt', {
        text: trimmed,
        workspace: workspaceRef.current,
        sessionId: sessionRef.current,
        model: agentRef.current.model,
      })
      if (!activeRef.current) return
      updateFeed(current => ({ ...current, sessionId: result.sessionId }))
    } catch (error) {
      updateFeed(current => ({ ...current, busy: false, startedAt: null, error: cleanError(error) }))
      if (!draftRef.current) replaceDraft(trimmed)
    } finally {
      operationRef.current = false
      if (cancelPendingRef.current && busyRef.current && activeRef.current) {
        void client.request('cancelTurn', { sessionId: sessionRef.current, workspace: workspaceRef.current }).catch(error => addNotice(cleanError(error), 'error'))
      }
      if (!busyRef.current) void refreshState()
      drainQueue()
    }
  }
  sendRef.current = send

  async function decide(granted: boolean, remember = false) {
    const item = pendingApproval(feedRef.current.items)
    if (!item?.approvalId || approvalRef.current) return
    approvalRef.current = item.approvalId
    try {
      const resolved = await client.request<boolean>('resolveApproval', {
        approvalId: item.approvalId,
        granted,
        sessionId: sessionRef.current,
        ...(remember && granted ? { remember: 'session' } : {}),
      })
      if (!resolved) {
        approvalRef.current = null
        updateFeed(current => ({ ...current, items: current.items.map(row => row.approvalId === item.approvalId
          ? { ...row, approvalId: undefined, granted: false } : row) }))
        addNotice('审批已失效', 'warning')
      }
    } catch (error) {
      approvalRef.current = null
      addNotice(cleanError(error), 'error')
    }
  }

  function moveCaret(nextCaret: number) {
    const at = clamp(nextCaret, draftRef.current)
    setCaret(at)
    caretRef.current = at
  }

  function replaceDraft(next: string, nextCaret = next.length) {
    setDraft(next)
    setCaret(clamp(nextCaret, next))
    caretRef.current = clamp(nextCaret, next)
    draftRef.current = next
    setSlashIndex(0)
    slashIndexRef.current = 0
    slashNavigatedRef.current = false
  }

  function pick(option: Suggestion, completeOnly = false) {
    const result = applySuggestion(draftRef.current, option)
    if (result.kind === 'fill' || completeOnly) {
      replaceDraft(result.text)
      return
    }
    replaceDraft('')
    void send(result.text)
  }

  useTerminalInput((input, key, pasted) => {
    const waiting = pendingApproval(feedRef.current.items)
    const text = draftRef.current
    const at = clamp(caretRef.current, text)
    const matches = slashSuggestions(text, { commands, models, sessions, skills })
    const slashIndex = slashIndexRef.current

    if (pasted) {
      if (readyRef.current) replaceDraft(insertAt(text, at, input), at + input.length)
      return
    }

    if (key.ctrl && input === 'c') {
      if (busyRef.current) {
        cancelPendingRef.current = true
        drainAllowedRef.current = false
        void client.request('cancelTurn', { sessionId: sessionRef.current, workspace: workspaceRef.current }).catch(error => addNotice(cleanError(error), 'error'))
        return
      }
      if (text) {
        replaceDraft('')
        return
      }
      if (!readyRef.current || Date.now() - ctrlCRef.current < 2000) {
        exit()
        return
      }
      ctrlCRef.current = Date.now()
      addNotice('再按一次 Ctrl+C 退出')
      return
    }
    if (key.ctrl && input === 'd') {
      exit()
      return
    }
    if (key.ctrl && input === 'q') { exit(); return }
    if (key.pageUp || key.pageDown) {
      transcriptRef.current?.scrollBy((key.pageUp ? -1 : 1) * Math.max(1, terminal.rows - 5))
      return
    }
    if (key.ctrl && key.end) {
      transcriptRef.current?.returnToLatest()
      return
    }
    if (key.ctrl && key.home) {
      transcriptRef.current?.scrollBy(-Number.MAX_SAFE_INTEGER)
      return
    }
    if (!readyRef.current) return
    ctrlCRef.current = 0
    if (key.escape) {
      if (waiting) {
        void decide(false)
        return
      }
      if (matches.length && text.includes(' ') && isSubmenu(text.split(/\s/, 1)[0])) {
        replaceDraft(`${text.split(/\s/, 1)[0]}`)
        return
      }
      if (text.startsWith('/')) replaceDraft('')
      return
    }
    if (waiting && !text) {
      const letter = input.toLowerCase()
      if (letter === 'y') { void decide(true); return }
      if (letter === 'a') { void decide(true, true); return }
      if (letter === 'n') { void decide(false); return }
    }
    if (key.return) {
      if (key.shift || (key.ctrl && input === 'j')) {
        const next = insertAt(text, at, '\n')
        replaceDraft(next, at + 1)
        return
      }
      const suggestions = slashSuggestions(text, { commands, models, sessions, skills })
      const exact = suggestions.find(item => item.kind === 'value'
        && text.trim().toLowerCase().endsWith(` ${item.value.toLowerCase()}`))
      const selected = suggestions[Math.min(slashIndex, suggestions.length - 1)]
      const argument = text.slice(text.search(/\s/) + 1).trim().toLowerCase()
      if (suggestions.length && (!text.includes(' ') || !text.trim().includes(' '))) {
        pick(suggestions[Math.min(slashIndex, suggestions.length - 1)])
        return
      }
      if (selected?.kind === 'value' && !exact
        && (slashNavigatedRef.current || text.endsWith(' ') || selected.value.toLowerCase().startsWith(argument))) {
        pick(selected)
        return
      }
      replaceDraft('')
      if (!text.trim() && !busyRef.current && queuedRef.current.length) {
        const [next, ...rest] = queuedRef.current
        updateQueue(rest)
        void send(next)
        return
      }
      void send(text)
      return
    }
    if (key.tab && matches.length) {
      pick(matches[Math.min(slashIndex, matches.length - 1)], true)
      return
    }
    if (key.upArrow) {
      if (matches.length) {
        slashIndexRef.current = (slashIndex - 1 + matches.length) % matches.length
        slashNavigatedRef.current = true
        setSlashIndex(slashIndexRef.current)
        return
      }
      moveCaret(moveVertical(text, at, -1))
      return
    }
    if (key.downArrow) {
      if (matches.length) {
        slashIndexRef.current = (slashIndex + 1) % matches.length
        slashNavigatedRef.current = true
        setSlashIndex(slashIndexRef.current)
        return
      }
      moveCaret(moveVertical(text, at, 1))
      return
    }
    if (key.leftArrow) {
      moveCaret(key.ctrl || key.meta ? prevWord(text, at) : prevBoundary(text, at))
      return
    }
    if (key.rightArrow) {
      moveCaret(key.ctrl || key.meta ? nextWord(text, at) : nextBoundary(text, at))
      return
    }
    if (key.home || (key.ctrl && input === 'a')) {
      moveCaret(key.ctrl && input === 'a' ? 0 : lineStart(text, at))
      return
    }
    if (key.end || (key.ctrl && input === 'e')) {
      moveCaret(key.ctrl && input === 'e' ? text.length : lineEnd(text, at))
      return
    }
    if (key.backspace || input === '\x7f' || input === '\b') {
      const next = deleteBackward(text, at)
      replaceDraft(next.text, next.caret)
      return
    }
    if (key.delete) {
      const next = deleteForward(text, at)
      replaceDraft(next.text, next.caret)
      return
    }
    if (key.ctrl && input === 'o') {
      const tool = lastExpandableTool(feedRef.current.items, lastUserIndex(feedRef.current.items))
      if (tool) {
        setExpanded(current => {
          const next = new Set(current)
          if (next.has(tool.id)) next.delete(tool.id)
          else next.add(tool.id)
          return next
        })
      }
      return
    }
    if (key.ctrl && input === 'j') {
      const next = insertAt(text, at, '\n')
      replaceDraft(next, at + 1)
      return
    }
    if (input && !key.ctrl && !key.meta) {
      const chunk = terminalText(input)
      const next = insertAt(text, at, chunk)
      replaceDraft(next, at + chunk.length)
    }
  }, event => {
    if (event.released) return
    if ((event.button & 64) && !(event.button & 2)) {
      transcriptRef.current?.scrollBy((event.button & 1 ? 1 : -1) * 3)
      return
    }
    if ((event.button & 3) !== 0 || !returnButtonRef.current?.yogaNode) return
    const button = returnButtonRef.current.yogaNode
    let left = 0
    let top = 0
    let node: DOMElement | null = returnButtonRef.current
    while (node?.yogaNode) {
      left += node.yogaNode.getComputedLeft()
      top += node.yogaNode.getComputedTop()
      node = node.parentNode || null
    }
    if (event.x - 1 >= left && event.x - 1 < left + button.getComputedWidth()
      && event.y - 1 >= top && event.y - 1 < top + button.getComputedHeight()) {
      transcriptRef.current?.returnToLatest()
    }
  })

  const waiting = pendingApproval(feed.items)
  const layout = useMemo(() => layoutTranscript(feed.items, !feed.busy), [feed.items, feed.busy])
  const frozenItems = useMemo<FrozenItem[]>(() => {
    const items: FrozenItem[] = []
    if (banner) items.push({ kind: 'banner', ...banner })
    items.push(...layout.frozen)
    return items
  }, [banner, layout.frozen])
  const visibleItems = [...frozenItems, ...layout.live]
  const promptHeight = Math.min(Math.max(1, Math.min(6, Math.floor(terminal.rows / 3))),
    promptRows(draft, caret, terminal.columns - 4).rows.length) + 2 + (queued.length ? 1 : 0)
  const menuHeight = draft.startsWith('/')
    ? Math.min(matches.length || 1, Math.max(1, Math.min(8, Math.floor(terminal.rows / 3)))) + 2 : 0
  const connecting = !ready && !feed.error
  const liveHeight = Math.max(0, terminal.rows - promptHeight - menuHeight - (feed.busy && !waiting ? 4 : 0) - 1 - (connecting ? 1 : 0) - (scrolled ? 1 : 0))
  const menuTitle = matches[0]?.kind === 'value'
    ? SUBMENU_TITLES[draft.split(/\s/, 1)[0].toLowerCase()]
    : '命令'

  return (
    <Box flexDirection="column" width={terminal.columns} height={terminal.rows} overflowY="hidden">
      <TranscriptViewport height={liveHeight} scrollRef={transcriptRef} onScrollChange={setScrolled}>
        {visibleItems.map((item, index) => (
          <Box key={item.kind === 'banner' ? 'banner' : `${item.id}-${index}`} flexShrink={0} flexDirection="column">
            <FeedRow item={item} now={item.kind === 'tool' || item.kind === 'thought' ? (item.endedAt || now) : now} expanded={expanded} />
          </Box>
        ))}
        {feed.error && feed.items.at(-1)?.text !== feed.error ? <Notice text={feed.error} tone="error" /> : null}
      </TranscriptViewport>
      <Box flexDirection="column" flexShrink={0}>
        {feed.busy && !waiting ? (
          <Spinner
            startedAt={feed.startedAt || now}
            now={now}
            tokens={Math.max(0, (agent.usage?.output_tokens || 0) - outputStart)}
            hint={Boolean(lastExpandableTool(feed.items, lastUserIndex(feed.items)))}
          />
        ) : null}
        {draft.startsWith('/') ? <SlashMenu items={matches} selected={slashIndex} title={menuTitle} /> : null}
        {scrolled ? (
          <Box justifyContent="center" flexShrink={0}>
            <Box ref={returnButtonRef}>
              <Text color={ACCENT} bold inverse> ↓ 返回最新 </Text>
            </Box>
          </Box>
        ) : null}
        <Prompt value={draft} caret={caret} busy={feed.busy} waiting={Boolean(waiting)}
          queued={queued.length ? `已排队 ${queued.length} 条：${queued[0].split('\n')[0]}${!feed.busy ? ' · Enter 发送' : ''}` : undefined} />
        <StatusLine state={agent} />
        {connecting ? <Text dimColor>正在连接运行核心…</Text> : null}
      </Box>
    </Box>
  )
}
