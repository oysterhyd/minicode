import { useLayoutEffect, useRef, useState, type RefObject } from 'react'
import { AnimatePresence, motion } from 'motion/react'
import { ArrowUp, AtSign, CircleAlert, Cpu, FileUp, ListPlus, Shield, ShieldCheck, ShieldOff, Square, X } from 'lucide-react'
import type { AgentState, Capabilities, Command, Model, Session } from '../../types'
import { promptHistory } from '../../lib/prefs'
import { modKey } from '../../lib/format'
import { SelectMenu } from './SelectMenu'
import { ContextMeter } from './ContextMeter'
import { fileSuggestions, isSubmenu, slashSuggestions, SuggestionList, type Suggestion } from './Suggestions'
import { RunStatus } from './RunStatus'

export const permissionOptions = [
  { value: 'default', label: '逐项确认', detail: '写入与命令执行前请求批准' },
  { value: 'accept_edits', label: '自动编辑', detail: '文件编辑自动批准，命令仍需确认' },
  { value: 'bypass', label: '全部允许', detail: '自动批准所有工具操作' },
]
export const effortOptions = [['off', '关闭'], ['low', '低'], ['medium', '中'], ['high', '高'], ['xhigh', '很高'], ['max', '最高']]

type Props = {
  textarea: RefObject<HTMLTextAreaElement | null>
  draft: string; setDraft: (value: string | ((previous: string) => string)) => void
  workspace: string | null; busy: boolean; loading: boolean; awaitingApproval: boolean; startedAt: number | null
  error: string; onDismissError: () => void; queue: string[]; onRemoveQueued: (index: number) => void
  state: AgentState; models: Model[]; commands: Command[]; sessions: Session[]; capabilities: Capabilities | null; files: string[]
  sendKey: 'enter' | 'mod-enter'; home: boolean
  onSubmit: (text: string) => void; onStop: () => void; onSlash: (command: string) => void
  onModel: (value: string) => void; onEffort: (value: string) => void; onPermission: (value: string) => void
  onRefreshState: () => void; onCompact: () => void
}

export function Composer(props: Props) {
  const { textarea, draft, setDraft } = props
  const [dismissed, setDismissed] = useState(false)
  const [active, setActive] = useState(0)
  const [menu, setMenu] = useState<'model' | 'permission' | null>(null)
  const [dragging, setDragging] = useState(false)
  const history = useRef<{ index: number; saved: string } | null>(null)
  const [caretPosition, setCaret] = useState<number | null>(null)
  const caret = { current: Math.min(caretPosition ?? draft.length, draft.length) }

  const mention = draft.slice(0, caret.current).match(/(?:^|\s)@([^\s@]*)$/)?.[1]
  const files = !dismissed && mention !== undefined && !draft.startsWith('/') ? fileSuggestions(mention, props.files) : []
  const slash = !dismissed ? slashSuggestions(draft, props) : []
  const list = files.length ? files : slash
  const index = Math.min(active, Math.max(0, list.length - 1))
  const listId = files.length ? 'file-suggestions' : 'slash-suggestions'

  useLayoutEffect(() => {
    const input = textarea.current
    if (!input) return
    input.style.height = 'auto'
    const max = Math.max(120, Math.min(320, window.innerHeight * .32))
    input.style.height = `${Math.min(max, Math.max(props.home ? 64 : 44, input.scrollHeight))}px`
  }, [draft, props.home])

  function insertFile(file: string) {
    const before = draft.slice(0, caret.current).replace(/@[^\s@]*$/, `@${file} `)
    const after = draft.slice(caret.current)
    setDraft(before + after); setCaret(before.length)
    requestAnimationFrame(() => { textarea.current?.focus(); textarea.current?.setSelectionRange(before.length, before.length) })
  }
  function pick(option: Suggestion) {
    if (option.kind === 'file') { insertFile(option.value); return }
    if (option.kind === 'command' && isSubmenu(option.value)) { setDraft(option.value + ' '); setActive(0); textarea.current?.focus(); return }
    const command = option.kind === 'value' ? `${draft.split(/\s/, 1)[0]} ${option.value}` : option.value
    setDraft(''); props.onSlash(command)
  }
  function submit() {
    const text = draft.trim()
    if (!text || props.loading) return
    history.current = null
    if (text.startsWith('/')) { setDraft(''); props.onSlash(text); return }
    if (!props.workspace) return
    props.onSubmit(text)
  }
  async function dropFiles(list: FileList) {
    const paths: string[] = []
    for (const file of Array.from(list)) {
      const absolute = window.desktop.getPathForFile?.(file) || ''
      if (!absolute) continue
      const relative = await window.desktop.request<string | null>('relativePath', { path: absolute, workspace: props.workspace }).catch(() => null)
      paths.push(relative || absolute)
    }
    if (paths.length) setDraft(value => `${value}${value && !value.endsWith(' ') ? ' ' : ''}${paths.map(path => `@${path.includes(' ') ? `"${path}"` : path}`).join(' ')} `)
    textarea.current?.focus()
  }

  const supportsEffort = props.models.find(model => model.id === props.state.model)?.supportsEffort ?? false
  const canSend = !props.loading && Boolean(draft.trim()) && (Boolean(props.workspace) || draft.startsWith('/'))
  const stopping = props.busy && !draft.trim()
  const PermissionIcon = props.state.permissionMode === 'bypass' ? ShieldOff : props.state.permissionMode === 'accept_edits' ? ShieldCheck : Shield
  function onKeyDown(event: React.KeyboardEvent<HTMLTextAreaElement>) {
    if (event.nativeEvent.isComposing || event.keyCode === 229) return
    if (list.length && ['ArrowDown', 'ArrowUp'].includes(event.key)) {
      event.preventDefault(); setActive((index + (event.key === 'ArrowDown' ? 1 : -1) + list.length) % list.length); return
    }
    if (list.length && (event.key === 'Tab' || (event.key === 'Enter' && !event.shiftKey))) { event.preventDefault(); pick(list[index]); return }
    if (event.key === 'Escape') { if (list.length || menu) event.stopPropagation(); setMenu(null); setDismissed(true); return }
    const input = event.currentTarget
    if ((event.key === 'ArrowUp' || event.key === 'ArrowDown') && !event.shiftKey && input.selectionStart === input.selectionEnd
      && (history.current || !draft) && (event.key === 'ArrowDown' || input.selectionStart === 0)) {
      const entries = promptHistory()
      const current = history.current?.index ?? -1
      const next = Math.max(-1, Math.min(entries.length - 1, current + (event.key === 'ArrowUp' ? 1 : -1)))
      if (next === current || (!entries.length)) return
      event.preventDefault()
      if (!history.current) history.current = { index: next, saved: draft }
      history.current.index = next
      setDraft(next < 0 ? history.current.saved : entries[next])
      if (next < 0) history.current = null
      return
    }
    const modifier = event.ctrlKey || event.metaKey
    if (event.key === 'Enter' && (props.sendKey === 'enter' ? !event.shiftKey && !modifier : modifier)) { event.preventDefault(); submit() }
  }

  return <div className={`composer-area ${props.home ? 'is-home' : ''}`}>
    <AnimatePresence initial={false}>{props.error && <motion.div className="error-banner" role="alert" initial={{ opacity: 0, y: 6, height: 0 }} animate={{ opacity: 1, y: 0, height: 'auto' }} exit={{ opacity: 0, height: 0 }}>
      <CircleAlert size={15} /><span>{props.error}</span><button className="icon-button icon-button-small" aria-label="关闭提示" onClick={props.onDismissError}><X size={13} /></button>
    </motion.div>}</AnimatePresence>
    <AnimatePresence initial={false}>{props.busy && !props.home && <RunStatus key="status" startedAt={props.startedAt} approval={props.awaitingApproval} onStop={props.onStop} />}</AnimatePresence>
    <AnimatePresence initial={false}>{props.queue.length > 0 && <motion.div className="queue-list" aria-label="排队消息" initial={{ opacity: 0, height: 0 }} animate={{ opacity: 1, height: 'auto' }} exit={{ opacity: 0, height: 0 }}>
      <div className="queue-heading">等待执行 · {props.queue.length}</div>
      <AnimatePresence initial={false}>{props.queue.map((text, i) => <motion.div layout className="queue-item" key={`${i}-${text}`} initial={{ opacity: 0, x: -6 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0, x: 10 }}>
        <ListPlus size={13} /><span title={text}>{text}</span>
        <button className="icon-button icon-button-small" aria-label={`取消排队消息 ${i + 1}`} onClick={() => props.onRemoveQueued(i)}><X size={12} /></button>
      </motion.div>)}</AnimatePresence>
    </motion.div>}</AnimatePresence>
    <motion.div layoutId="task-composer" transition={{ type: 'spring', stiffness: 380, damping: 38 }}
      className={`composer ${props.busy ? 'is-busy' : ''} ${dragging ? 'is-dragging' : ''}`}
      onDragOver={event => { if (event.dataTransfer.types.includes('Files') && props.workspace) { event.preventDefault(); setDragging(true) } }}
      onDragLeave={event => { if (!event.currentTarget.contains(event.relatedTarget as Node)) setDragging(false) }}
      onDrop={event => { event.preventDefault(); setDragging(false); if (event.dataTransfer.files.length) void dropFiles(event.dataTransfer.files) }}>
      <AnimatePresence>{list.length > 0 && <SuggestionList key={listId} id={listId} label={files.length ? '引用本地文件' : '命令建议'}
        hint={files.length ? '↑↓ 选择 · Tab 插入' : '↑↓ 选择 · Enter 执行'} items={list} active={index} onPick={pick} onHover={setActive} />}</AnimatePresence>
      <AnimatePresence>{dragging && <motion.div className="drop-overlay" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }}><FileUp size={18} />松开以引用文件</motion.div>}</AnimatePresence>
      <textarea ref={textarea} value={draft} rows={1} aria-label="任务输入" spellCheck={false}
        placeholder={props.workspace ? props.busy ? '追加指令，将在当前回合结束后执行…' : '描述任务，输入 / 使用命令，@ 引用文件' : '先打开工作区，再描述你想完成的任务…'}
        aria-controls={list.length ? listId : undefined} aria-activedescendant={list.length ? `${listId}-${index}` : undefined}
        onChange={event => { setDraft(event.target.value); setCaret(event.target.selectionStart); setDismissed(false); setActive(0); history.current = null }}
        onSelect={event => setCaret(event.currentTarget.selectionStart)} onKeyDown={onKeyDown} />
      <div className="composer-toolbar">
        <button className="composer-icon" aria-label="引用文件" data-tip="引用文件" data-shortcut="@" disabled={!props.workspace} onClick={() => {
          const next = draft + (draft && !draft.endsWith(' ') ? ' @' : '@')
          setDraft(next); setCaret(next.length); setDismissed(false); setActive(0); textarea.current?.focus()
        }}><AtSign size={16} /></button>
        <SelectMenu label="切换模型" tip="模型与思考强度" value={props.state.model} icon={<Cpu size={13} />} open={menu === 'model'} onOpen={open => setMenu(open ? 'model' : null)} onSelect={props.onModel}
          options={props.models.map(option => ({ value: option.id, label: option.name || option.id, detail: option.available ? option.provider : '尚未配置', disabled: !option.available && option.id !== props.state.model }))}
          secondary={{ label: '思考强度', value: props.state.effort, onSelect: props.onEffort, hint: supportsEffort ? undefined : '当前模型不支持',
            options: effortOptions.map(([value, label]) => ({ value, label, disabled: !supportsEffort })) }} />
        <SelectMenu label="权限模式" value={props.state.permissionMode} icon={<PermissionIcon size={13} />} open={menu === 'permission'} onOpen={open => setMenu(open ? 'permission' : null)} onSelect={props.onPermission} options={permissionOptions} />
        <span className="toolbar-spacer" />
        <ContextMeter used={props.state.contextTokens} windowSize={props.state.contextWindow} breakdown={props.state.contextBreakdown || { system: 0, tools: 0, messages: 0 }}
          usage={props.state.usage} statistics={props.state.statistics} onOpen={props.onRefreshState} onCompact={props.state.sessionId && !props.busy ? props.onCompact : undefined} />
        <motion.button whileTap={{ scale: .92 }} className={`send-button ${stopping ? 'is-stop' : ''} ${canSend || stopping ? 'is-ready' : ''}`}
          onClick={stopping ? props.onStop : submit} disabled={!stopping && !canSend}
          aria-label={props.busy ? draft.trim() ? '加入队列' : '停止任务' : '发送任务'}
          data-tip={props.busy ? draft.trim() ? '加入队列' : '停止任务' : '发送'} data-shortcut={stopping ? `${modKey} .` : props.sendKey === 'enter' ? 'Enter' : `${modKey} Enter`}>
          <AnimatePresence initial={false} mode="popLayout"><motion.span key={stopping ? 'stop' : props.busy ? 'queue' : 'send'} className="send-icon"
            initial={{ scale: .5, opacity: 0, rotate: -30 }} animate={{ scale: 1, opacity: 1, rotate: 0 }} exit={{ scale: .5, opacity: 0 }} transition={{ duration: .16 }}>
            {stopping ? <Square size={11} fill="currentColor" /> : props.busy ? <ListPlus size={16} /> : <ArrowUp size={17} strokeWidth={2.4} />}
          </motion.span></AnimatePresence>
        </motion.button>
      </div>
    </motion.div>
  </div>
}
