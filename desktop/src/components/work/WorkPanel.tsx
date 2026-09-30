import { useEffect, useMemo, useState } from 'react'
import { AnimatePresence, motion } from 'motion/react'
import { Check, CheckCheck, CircleDashed, FileDiff, Files, FolderOpen, ListTodo, LoaderCircle, PanelRightClose, PanelRightOpen, Plus, RefreshCw, Search, Terminal, X } from 'lucide-react'
import type { AgentState, Change, FeedItem, SessionTask } from '../../types'
import { basename, dirname, duration, modKey } from '../../lib/format'
import { DiffStat } from '../DiffView'
import { FileIcon, FileTree } from './FileTree'
import { Preview, type PreviewTarget } from './Preview'
import { SessionUsage } from './SessionUsage'
import { useToast } from '../ui/Toast'

type Tab = 'changes' | 'files' | 'terminal' | 'tasks'
const tabs = [['changes', FileDiff, '改动'], ['files', Files, '文件'], ['terminal', Terminal, '终端'], ['tasks', ListTodo, '任务']] as const
type Props = {
  workspace: string | null; changes: Change[]; files: string[]; items: FeedItem[]; tasks: SessionTask[]; state: AgentState
  refresh: () => Promise<unknown> | void; collapsed: boolean; onToggle: () => void; onMention: (path: string) => void
}

function statusLetter(status: string) {
  if (status === '??') return 'U'
  if (status.includes('D')) return 'D'
  if (status.includes('A')) return 'A'
  if (status.includes('R')) return 'R'
  return 'M'
}

export function WorkPanel({ workspace, changes, files, items, tasks, state, refresh, collapsed, onToggle, onMention }: Props) {
  const [tab, setTab] = useState<Tab>('changes')
  const [target, setTarget] = useState<PreviewTarget | null>(null)
  const [refreshing, setRefreshing] = useState(false)
  const [staging, setStaging] = useState(false)
  const [query, setQuery] = useState('')
  const toast = useToast()
  const shellItems = items.filter(item => item.kind === 'tool' && item.name === 'bash')
  const changed = useMemo(() => new Map(changes.map(change => [change.path, statusLetter(change.status)])), [changes])
  const totals = changes.reduce((sum, change) => ({ additions: sum.additions + (change.additions || 0), deletions: sum.deletions + (change.deletions || 0) }), { additions: 0, deletions: 0 })
  const done = tasks.filter(task => task.status === 'done').length
  useEffect(() => { setTarget(null); setQuery('') }, [workspace])
  useEffect(() => {
    const open = (event: Event) => { const detail = (event as CustomEvent<Tab>).detail; if (detail) { setTab(detail); setTarget(null) } }
    window.addEventListener('minicode:work-tab', open)
    return () => window.removeEventListener('minicode:work-tab', open)
  }, [])

  function selectTab(value: Tab) { setTab(value); setTarget(null) }
  async function reload() {
    setRefreshing(true)
    try { await refresh() } finally { setRefreshing(false) }
  }
  async function stageAll() {
    setStaging(true)
    try { await window.desktop.request('stageAll', { workspace }); await refresh(); toast({ tone: 'success', title: `已暂存 ${changes.length} 个文件` }) }
    catch (e) { toast({ tone: 'error', title: '暂存失败', detail: String(e).replace(/^Error: /, '') }) }
    finally { setStaging(false) }
  }

  if (collapsed) return <aside className="rail work-rail" aria-label="已收拢的工作区">
    <button className="rail-button" onClick={onToggle} aria-label="展开工作区" data-tip="展开工作台" data-shortcut={`${modKey} Shift B`}><PanelRightOpen size={17} /></button>
    {tabs.map(([id, Icon, label]) => <button className="rail-button" key={id} onClick={() => { selectTab(id); onToggle() }} aria-label={`查看${label}`} data-tip={label}>
      <Icon size={16} />{id === 'changes' && changes.length > 0 && <span className="rail-badge">{changes.length}</span>}
      {id === 'tasks' && tasks.length > 0 && <span className="rail-badge">{done}/{tasks.length}</span>}
    </button>)}
  </aside>

  return <aside className="work-panel" aria-label="工作台">
    <div className="work-tabs" role="tablist" aria-label="工作台视图" onKeyDown={event => {
      if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return
      event.preventDefault()
      const index = tabs.findIndex(([id]) => id === tab)
      const next = event.key === 'Home' ? 0 : event.key === 'End' ? tabs.length - 1 : (index + (event.key === 'ArrowRight' ? 1 : -1) + tabs.length) % tabs.length
      selectTab(tabs[next][0]); document.getElementById(`work-tab-${tabs[next][0]}`)?.focus()
    }}>
      {tabs.map(([id, Icon, label]) => <button id={`work-tab-${id}`} key={id} role="tab" aria-selected={tab === id} aria-controls="work-tab-panel" tabIndex={tab === id ? 0 : -1}
        className={`work-tab ${tab === id ? 'is-active' : ''}`} onClick={() => selectTab(id)}>
        {tab === id && <motion.span className="work-tab-indicator" layoutId="work-tab-indicator" transition={{ type: 'spring', stiffness: 520, damping: 40 }} />}
        <Icon size={13} /><span>{label}</span>
        {id === 'changes' && changes.length > 0 && <span className="tab-count">{changes.length}</span>}
        {id === 'tasks' && tasks.length > 0 && <span className="tab-count">{done}/{tasks.length}</span>}
      </button>)}
      <span className="toolbar-spacer" />
      <button className="icon-button icon-button-small" aria-label="刷新工作区" data-tip="刷新" disabled={refreshing} onClick={() => void reload()}><RefreshCw size={13} className={refreshing ? 'spin' : ''} /></button>
      <button className="icon-button icon-button-small" aria-label="收拢工作区" data-tip="收拢工作台" data-shortcut={`${modKey} Shift B`} onClick={onToggle}><PanelRightClose size={15} /></button>
    </div>
    <div className="work-body" id="work-tab-panel" role="tabpanel" aria-labelledby={`work-tab-${tab}`}>
      <AnimatePresence mode="wait" initial={false}><motion.div className="work-view" key={`${tab}-${target?.path || 'list'}`}
        initial={{ opacity: 0, x: target ? 10 : -6 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0, x: target ? -6 : 6 }} transition={{ duration: .14, ease: [.2, .8, .2, 1] }}>
        {target ? <Preview target={target} workspace={workspace} version={changes} onBack={() => setTarget(null)} onMention={onMention} onChanged={refresh} />
          : tab === 'changes' ? <div className="work-list scrollbar">
            <div className="work-section-label"><span>未提交的改动</span>{changes.length > 0 && <DiffStat {...totals} />}
              {changes.length > 0 && <button className="button button-ghost button-small" disabled={staging} onClick={() => void stageAll()}>{staging ? <LoaderCircle size={12} className="spin" /> : <Plus size={12} />}全部暂存</button>}</div>
            {!changes.length && <div className="work-empty"><span className="work-empty-icon">{workspace ? <CheckCheck size={22} /> : <FolderOpen size={22} />}</span>
              <strong>{workspace ? '工作区保持整洁' : '还没有工作区'}</strong><p>{workspace ? 'Agent 修改文件后，可以在这里逐个审查差异。' : '打开本地项目后，在这里查看文件和改动。'}</p></div>}
            {changes.map(change => <button className="file-row" key={change.path} onClick={() => setTarget({ kind: 'diff', path: change.path, status: change.status })}
              aria-label={`${basename(change.path)} ${dirname(change.path) || '项目根目录'} ${statusLetter(change.status)}`}>
              <FileIcon path={change.path} /><span className="file-row-label"><strong>{basename(change.path)}</strong><small>{dirname(change.path) || '项目根目录'}</small></span>
              <DiffStat additions={change.additions} deletions={change.deletions} />
              <span className={`file-status status-${statusLetter(change.status)}`}>{statusLetter(change.status)}</span>
            </button>)}
          </div>
          : tab === 'files' ? <div className="work-files">
            <label className="file-search"><Search size={13} /><input aria-label="搜索文件" placeholder="按名称搜索文件…" value={query} onChange={event => setQuery(event.target.value)} />
              {query && <button aria-label="清空文件搜索" onClick={() => setQuery('')}><X size={12} /></button>}</label>
            <div className="work-list scrollbar">
              {files.length ? <FileTree files={files} query={query} changed={changed} onOpen={path => setTarget({ kind: 'file', path })} />
                : <div className="work-empty"><Files size={22} /><strong>暂无项目文件</strong><p>打开工作区后，文件将显示在这里。</p></div>}
            </div>
          </div>
          : tab === 'terminal' ? <div className="work-list scrollbar terminal-panel">
            <div className="work-section-label"><span>命令执行记录</span><span>{shellItems.length}</span></div>
            {!shellItems.length && <div className="work-empty"><Terminal size={22} /><strong>终端已就绪</strong><p>Agent 执行命令时，输出会实时显示在这里。</p></div>}
            {shellItems.map(item => <div key={item.id} className={`terminal-entry ${item.pending ? 'is-running' : item.success === false ? 'is-failed' : ''}`}>
              <div className="terminal-command"><span className="terminal-prompt">$</span><code>{String(item.args?.command || '')}</code>
                {item.pending ? <LoaderCircle size={12} className="spin" /> : item.success ? <Check size={12} /> : <X size={12} />}
                {item.startedAt && item.endedAt && <time>{duration(item.endedAt - item.startedAt)}</time>}</div>
              <pre>{item.output || (item.pending ? '正在运行…' : '无输出')}</pre>
            </div>)}
          </div>
          : <div className="work-list scrollbar">
            <div className="work-section-label"><span>任务进度</span><span>{done}/{tasks.length}</span></div>
            {tasks.length > 0 && <div className="task-progress"><motion.span animate={{ width: `${100 * done / tasks.length}%` }} transition={{ type: 'spring', stiffness: 200, damping: 30 }} /></div>}
            {tasks.length ? tasks.map(task => <div className={`task-row task-${task.status}`} key={task.task_id}>
              <span className="task-check">{task.status === 'done' ? <Check size={11} /> : task.status === 'running' ? <LoaderCircle size={11} className="spin" /> : task.status === 'failed' ? <X size={11} /> : <CircleDashed size={11} />}</span>
              <span className="task-title" title={task.title}>{task.title}</span>
              <small>{{ pending: '待处理', ready: '已就绪', running: '进行中', done: '完成', failed: '失败' }[task.status]}</small>
            </div>) : <div className="work-empty"><ListTodo size={22} /><strong>还没有任务清单</strong><p>Agent 拆分复杂任务后，进度会显示在这里。</p></div>}
            <SessionUsage state={state} />
          </div>}
      </motion.div></AnimatePresence>
    </div>
  </aside>
}
