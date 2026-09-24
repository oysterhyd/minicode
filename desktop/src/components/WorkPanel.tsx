import { useEffect, useState } from 'react'
import { AnimatePresence, motion } from 'motion/react'
import { Activity, ArrowLeft, Check, CheckCheck, ChevronDown, FileCode2, Files, FileDiff, FolderOpen, ListTodo, LoaderCircle, PanelRightClose, PanelRightOpen, RefreshCw, Search, Terminal, X } from 'lucide-react'
import type { AgentEvent, AgentState, Change, FeedItem, SessionTask } from '../types'
import { CopyButton } from './CopyButton'

type Tab = 'changes' | 'files' | 'terminal'
type Props = {
  workspace: string | null; changes: Change[]; files: string[]; items: FeedItem[]; tasks: SessionTask[]
  state: AgentState; trace: AgentEvent[]; refresh: () => void | Promise<unknown>; collapsed: boolean; onToggle: () => void
}
const tabs = [['changes', FileDiff, '改动'], ['files', Files, '文件'], ['terminal', Terminal, '终端']] as const

function DiffText({ text }: { text: string }) {
  return <div className="diff-lines">{text.split('\n').map((line, index) => <div key={index}
    className={line.startsWith('+') && !line.startsWith('+++') ? 'diff-add' : line.startsWith('-') && !line.startsWith('---') ? 'diff-remove' : line.startsWith('@@') ? 'diff-hunk' : 'diff-context'}><span className="diff-line-number" aria-hidden="true">{index + 1}</span><span>{line || ' '}</span></div>)}</div>
}

export function WorkPanel({ workspace, changes, files, items, tasks, state, trace, refresh, collapsed, onToggle }: Props) {
  const [tab, setTab] = useState<Tab>('changes')
  const [selected, setSelected] = useState<string | null>(null)
  const [content, setContent] = useState('')
  const [confirmed, setConfirmed] = useState<string[]>([])
  const [error, setError] = useState('')
  const [loading, setLoading] = useState(false)
  const [staging, setStaging] = useState(false)
  const [refreshing, setRefreshing] = useState(false)
  const [query, setQuery] = useState('')
  const [tasksOpen, setTasksOpen] = useState(true)
  const [sessionOpen, setSessionOpen] = useState(true)
  const shellItems = items.filter(item => item.kind === 'tool' && item.name === 'bash')
  const contextPercent = Math.min(100, Math.round(100 * state.contextTokens / Math.max(1, state.contextWindow)))
  const toolCount = trace.filter(event => event.type === 'tool_call_start').length
  const visibleFiles = files.filter(file => file.toLowerCase().includes(query.toLowerCase()))

  useEffect(() => { setSelected(null); setContent(''); setConfirmed([]); setQuery(''); setError('') }, [workspace])
  useEffect(() => { setContent('') }, [selected, tab])
  useEffect(() => {
    if (!selected || tab === 'terminal') return
    let active = true
    setLoading(true); setError('')
    const method = tab === 'files' ? 'readFile' : 'diff'
    window.desktop.request<string>(method, { path: selected, workspace }).then(value => {
      if (active) { setContent(value); setError('') }
    }).catch(e => { if (active) setError(String(e)) }).finally(() => { if (active) setLoading(false) })
    return () => { active = false }
  }, [selected, tab, changes, workspace])

  function selectTab(value: Tab) { setTab(value); setSelected(null); setError(''); setLoading(false) }
  async function confirm() {
    if (!selected || staging) return
    setStaging(true)
    try {
      await window.desktop.request('confirmDiff', { path: selected, workspace })
      setConfirmed(value => [...value, selected]); await refresh()
    } catch (e) { setError(String(e)) }
    finally { setStaging(false) }
  }
  async function refreshFiles() {
    setRefreshing(true)
    try { await refresh() } catch (e) { setError(String(e)) }
    finally { setRefreshing(false) }
  }

  if (collapsed) return <aside className="rail work-rail" aria-label="已收拢的工作区">
    <button className="rail-button" onClick={onToggle} title="展开工作台" aria-label="展开工作区"><PanelRightOpen size={18} /></button>
    {tabs.map(([id, Icon, label]) => <button className={`rail-button ${tab === id ? 'rail-active' : ''}`} key={id} onClick={() => { selectTab(id); onToggle() }} title={`查看${label}`} aria-label={`查看${label}`}><Icon size={17} />{id === 'changes' && changes.length > 0 && <span className="rail-badge">{changes.length}</span>}</button>)}
  </aside>

  return <aside className="work-panel">
    <header className="work-header"><div><h2>工作台</h2><span title={workspace || ''}>{workspace ? workspace.split(/[\\/]/).pop() : '尚未打开目录'}</span></div><div className="flex"><button className="icon-button" aria-label="刷新工作区" title="刷新工作区" disabled={refreshing} onClick={refreshFiles}><RefreshCw size={15} className={refreshing ? 'animate-spin' : ''} /></button><button className="icon-button" aria-label="收拢工作区" title="收拢工作台 · Ctrl Shift B" onClick={onToggle}><PanelRightClose size={17} /></button></div></header>
    <div className="work-tabs" role="tablist" aria-label="工作台视图" onKeyDown={event => {
      if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return
      event.preventDefault()
      const index = tabs.findIndex(([id]) => id === tab)
      const next = event.key === 'Home' ? 0 : event.key === 'End' ? 2 : (index + (event.key === 'ArrowRight' ? 1 : -1) + 3) % 3
      selectTab(tabs[next][0]); document.getElementById(`work-tab-${tabs[next][0]}`)?.focus()
    }}>
      {tabs.map(([id, Icon, label]) => <button id={`work-tab-${id}`} key={id} role="tab" aria-selected={tab === id} aria-controls="work-tab-panel" tabIndex={tab === id ? 0 : -1} className={`work-tab ${tab === id ? 'work-tab-active' : ''}`} onClick={() => selectTab(id)}><Icon size={14} />{label}{id === 'changes' && changes.length > 0 && <span className="tab-count">{changes.length}</span>}{tab === id && <motion.span className="tab-indicator" layoutId="work-tab-indicator" transition={{ type: 'spring', stiffness: 500, damping: 38 }} />}</button>)}
    </div>
    <div className="work-body scrollbar" id="work-tab-panel" role="tabpanel" aria-labelledby={`work-tab-${tab}`}>
      <AnimatePresence mode="wait" initial={false}><motion.div key={`${tab}-${selected || 'list'}`} initial={{ opacity: 0, x: selected ? 8 : -5 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0 }} transition={{ duration: .14 }}>
        {selected && tab !== 'terminal' ? <>
          <div className="preview-toolbar"><button className="icon-button" aria-label="返回文件列表" onClick={() => setSelected(null)}><ArrowLeft size={15} /></button><span title={selected}>{selected.split('/').pop()}</span><CopyButton text={content} label="复制" /></div>
          <div className="preview-path" title={selected}>{selected}</div>
          {loading ? <div className="preview-loading" role="status"><LoaderCircle size={16} className="animate-spin" />正在读取文件…</div> : tab === 'changes' ? <DiffText text={content} /> : <pre className="file-preview">{content.split('\n').map((line, index) => <div key={index}><span className="file-line-number" aria-hidden="true">{index + 1}</span><span>{line || ' '}</span></div>)}</pre>}
          {tab === 'changes' && <div className="diff-actions"><button className="confirm-button" disabled={loading || staging || Boolean(error)} onClick={confirm}>{staging ? <LoaderCircle size={14} className="animate-spin" /> : confirmed.includes(selected) ? <CheckCheck size={14} /> : <Check size={14} />}{staging ? '正在暂存' : confirmed.includes(selected) ? '再次暂存' : '暂存此文件'}</button>{confirmed.includes(selected) && <span role="status">已暂存当前版本</span>}</div>}
        </> : tab === 'changes' ? <>
          <div className="work-section-label"><span>未提交的改动</span><span>{changes.length} 个文件</span></div>
          {!changes.length && <div className="work-empty"><span className="work-empty-icon">{workspace ? <CheckCheck size={25} /> : <FolderOpen size={25} />}</span><strong>{workspace ? '工作区保持整洁' : '还没有工作区'}</strong><p>{workspace ? '文件改动后，可以在这里审查差异。' : '打开本地项目后，在这里查看文件和改动。'}</p></div>}
          {changes.map(change => <button className="file-row" key={change.path} onClick={() => setSelected(change.path)} title={change.path}><FileCode2 size={16} /><span className="file-row-label"><strong>{change.path.split('/').pop()}</strong><small>{change.path.includes('/') ? change.path.slice(0, change.path.lastIndexOf('/')) : '项目根目录'}</small></span><span className={`file-status ${change.status.includes('D') ? 'file-deleted' : change.status === '??' || change.status.includes('A') ? 'file-added' : ''}`}>{change.status === '??' ? 'U' : change.status.trim()}</span></button>)}
          {changes.length > 0 && <p className="work-list-hint">选择文件，查看并审查改动</p>}
        </> : tab === 'files' ? <>
          <label className="file-search"><Search size={14} /><input aria-label="搜索文件" placeholder="搜索项目文件…" value={query} onChange={event => setQuery(event.target.value)} />{query && <button aria-label="清空文件搜索" onClick={() => setQuery('')}><X size={13} /></button>}</label>
          <div className="work-section-label"><span>项目文件</span><span>{visibleFiles.length}</span></div>
          {visibleFiles.map(file => <button className="file-row file-row-compact" key={file} onClick={() => setSelected(file)} title={file}><FileCode2 size={15} /><span className="truncate">{file}</span></button>)}
          {!visibleFiles.length && <div className="work-empty"><Files size={25} /><strong>{query ? '未找到匹配文件' : '暂无项目文件'}</strong><p>{query ? '尝试文件名或路径的一部分。' : '打开工作区后，文件将显示在这里。'}</p></div>}
        </> : <div className="terminal-panel"><div className="work-section-label"><span>命令执行记录</span><span>{shellItems.length}</span></div>
          {!shellItems.length && <div className="work-empty"><Terminal size={25} /><strong>终端已就绪</strong><p>Agent 执行命令时，输出会实时显示在这里。</p></div>}
          {shellItems.map(item => <div key={item.id} className="terminal-entry"><div className="terminal-command"><span>$</span><code>{String(item.args?.command || '')}</code>{item.pending ? <LoaderCircle size={13} className="animate-spin" /> : item.success ? <Check size={13} /> : <X size={13} />}</div><pre>{item.output || (item.pending ? '正在运行…' : '无输出')}</pre></div>)}
        </div>}
      </motion.div></AnimatePresence>
    </div>
    {error && <div className="work-error" role="alert"><span>{error}</span><button className="icon-button" aria-label="关闭错误" onClick={() => setError('')}><X size={13} /></button></div>}
    <section className="work-summary-section"><button className="work-summary-heading" onClick={() => setTasksOpen(value => !value)} aria-expanded={tasksOpen}><span><ListTodo size={15} />任务进度<span className="summary-count">{tasks.filter(task => task.status === 'done').length}/{tasks.length}</span></span><ChevronDown size={14} className={tasksOpen ? '' : '-rotate-90'} /></button>
      {tasksOpen && <div className="task-list scrollbar">{tasks.length ? tasks.map(task => <div className="task-row" key={task.task_id}><span className={`task-check ${task.status === 'done' ? 'task-done' : ''}`}>{task.status === 'done' ? <Check size={11} /> : task.status === 'running' ? <LoaderCircle size={11} className="animate-spin" /> : task.status === 'failed' ? <X size={11} /> : null}</span><span className={`task-title ${task.status === 'done' ? 'task-completed' : ''}`} title={task.title}>{task.title}</span><small>{{ pending: '待处理', ready: '已就绪', running: '进行中', done: '完成', failed: '失败' }[task.status]}</small></div>) : <p className="summary-empty">任务拆分后，进度将显示在这里。</p>}</div>}
    </section>
    <section className="work-summary-section"><button className="work-summary-heading" onClick={() => setSessionOpen(value => !value)} aria-expanded={sessionOpen}><span><Activity size={15} />会话用量</span><ChevronDown size={14} className={sessionOpen ? '' : '-rotate-90'} /></button>
      {sessionOpen && <div className="session-stats"><div className="stat-card"><div className="stat-label">输入 / 输出</div><div className="stat-value">{(state.usage?.input_tokens || 0).toLocaleString()}<span> / {(state.usage?.output_tokens || 0).toLocaleString()}</span></div></div><div className="stat-card"><div className="stat-label">上下文</div><div className="stat-value">{contextPercent}<span>%</span></div><div className="stat-track" role="meter" aria-label="上下文用量" aria-valuenow={contextPercent} aria-valuemin={0} aria-valuemax={100}><span style={{ transform: `scaleX(${contextPercent / 100})` }} /></div></div><div className="stat-inline"><span>对话轮次</span><strong>{state.rounds}</strong><span>工具调用</span><strong>{toolCount}</strong></div></div>}
    </section>
  </aside>
}
