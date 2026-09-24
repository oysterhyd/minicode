import { useEffect, useState } from 'react'
import { Activity, Check, ChevronDown, FileCode2, Files, FileDiff, ListTodo, PanelRightClose, PanelRightOpen, RefreshCw, Terminal, X } from 'lucide-react'
import type { AgentEvent, AgentState, Change, FeedItem, SessionTask } from '../types'

type Tab = 'changes' | 'files' | 'terminal'
type Props = {
  workspace: string | null
  changes: Change[]
  files: string[]
  items: FeedItem[]
  tasks: SessionTask[]
  state: AgentState
  trace: AgentEvent[]
  refresh: () => void
  collapsed: boolean
  onToggle: () => void
}

function DiffText({ text }: { text: string }) {
  return <div className="diff-lines font-mono text-[11px] leading-[19px]">{text.split('\n').map((line, index) => <div key={index}
    className={line.startsWith('+') && !line.startsWith('+++') ? 'diff-add' : line.startsWith('-') && !line.startsWith('---') ? 'diff-remove' : line.startsWith('@@') ? 'diff-hunk' : 'diff-context'}>{line || ' '}</div>)}</div>
}

export function WorkPanel({ workspace, changes, files, items, tasks, state, trace, refresh, collapsed, onToggle }: Props) {
  const [tab, setTab] = useState<Tab>('changes')
  const [selected, setSelected] = useState<string | null>(null)
  const [content, setContent] = useState('')
  const [confirmed, setConfirmed] = useState<string[]>([])
  const [error, setError] = useState('')
  const [tasksOpen, setTasksOpen] = useState(true)
  const [sessionOpen, setSessionOpen] = useState(true)
  const shellItems = items.filter(item => item.kind === 'tool' && item.name === 'bash')
  const contextPercent = Math.min(100, Math.round(100 * state.contextTokens / Math.max(1, state.contextWindow)))
  const toolCount = trace.filter(event => event.type === 'tool_call_start').length

  useEffect(() => { setSelected(null); setContent(''); setConfirmed([]) }, [workspace])
  useEffect(() => {
    if (!selected) return
    const method = tab === 'files' ? 'readFile' : 'diff'
    window.desktop.request<string>(method, { path: selected }).then(setContent).catch(e => setError(String(e)))
  }, [selected, tab, changes])

  async function confirm() {
    if (!selected) return
    try {
      await window.desktop.request('confirmDiff', { path: selected })
      setConfirmed(value => [...value, selected])
      refresh()
    } catch (e) { setError(String(e)) }
  }

  if (collapsed) return <aside className="rail border-l border-edge bg-work" aria-label="已收拢的工作区">
    <button className="rail-button mt-4" onClick={onToggle} title="展开工作区" aria-label="展开工作区"><PanelRightOpen size={19} /></button>
    <button className="rail-button mt-5" onClick={() => { setTab('changes'); onToggle() }} title="查看改动" aria-label="查看改动"><FileDiff size={18} /></button>
    <button className="rail-button mt-2" onClick={() => { setTab('files'); onToggle() }} title="查看文件" aria-label="查看文件"><Files size={18} /></button>
    <button className="rail-button mt-2" onClick={() => { setTab('terminal'); onToggle() }} title="查看终端" aria-label="查看终端"><Terminal size={18} /></button>
  </aside>

  return <aside className="work-panel flex h-full min-w-0 flex-col border-l border-edge bg-work">
    <div className="flex h-16 items-center justify-between border-b border-edge px-5"><div className="min-w-0"><div className="text-[14px] font-semibold text-white">工作区</div><div className="mt-0.5 max-w-[230px] truncate text-xs text-subtle" title={workspace || ''}>{workspace || '尚未绑定目录'}</div></div><div className="flex shrink-0"><button className="icon-button" aria-label="刷新工作区" onClick={refresh}><RefreshCw size={16} /></button><button className="icon-button" aria-label="收拢工作区" title="收拢工作区" onClick={onToggle}><PanelRightClose size={18} /></button></div></div>
    <div className="flex border-b border-edge px-3">
      {([['changes', FileDiff, '改动'], ['files', Files, '文件'], ['terminal', Terminal, '终端']] as const).map(([id, Icon, label]) => <button key={id} className={`work-tab ${tab === id ? 'work-tab-active' : ''}`} onClick={() => { setTab(id); setSelected(null) }}><Icon size={15} />{label}{id === 'changes' && changes.length > 0 && <span className="tab-count">{changes.length}</span>}</button>)}
    </div>
    <div className="scrollbar min-h-0 flex-1 overflow-y-auto">
      {tab === 'changes' && <>
        <div className="section-label flex items-center justify-between px-5 py-4"><span>未提交改动</span><span>{changes.length} 个文件</span></div>
        {!workspace && <div className="panel-empty">选择本地目录后查看改动</div>}
        {workspace && changes.length === 0 && <div className="panel-empty">没有待审查的 Git 改动</div>}
        {changes.map(change => <button className={`file-row ${selected === change.path ? 'file-selected' : ''}`} key={change.path} onClick={() => setSelected(change.path)}>
          <FileCode2 size={15} className="shrink-0 text-subtle" /><span className="min-w-0 flex-1 truncate text-left">{change.path}</span><span className="file-status">{change.status === '??' ? 'U' : change.status.trim()}</span>
        </button>)}
        {selected && <div className="mt-4 border-t border-edge"><div className="flex items-center justify-between gap-2 px-4 py-3"><span className="truncate text-xs font-medium text-white">{selected}</span><button className="confirm-button" onClick={confirm}>{confirmed.includes(selected) ? <Check size={13} /> : <FileDiff size={13} />}{confirmed.includes(selected) ? '已确认' : '暂存并确认'}</button></div><DiffText text={content} /></div>}
      </>}
      {tab === 'files' && <>
        <div className="section-label px-5 py-4">项目文件</div>
        {files.map(file => <button className={`file-row ${selected === file ? 'file-selected' : ''}`} key={file} onClick={() => setSelected(file)}><FileCode2 size={15} className="shrink-0 text-subtle" /><span className="truncate text-left">{file}</span></button>)}
        {selected && <div className="mt-4 border-t border-edge"><div className="px-4 py-3 text-xs font-medium text-white">{selected}</div><pre className="file-preview">{content}</pre></div>}
      </>}
      {tab === 'terminal' && <div className="p-4">
        <div className="mb-4 flex items-center gap-2 text-xs text-subtle"><Terminal size={14} /> Agent 命令执行记录</div>
        {shellItems.length === 0 && <div className="panel-empty">命令输出将在这里显示</div>}
        {shellItems.map(item => <div key={item.id} className="terminal-entry"><div className="mb-2 flex items-center gap-2 text-xs text-primary"><span>$</span><span className="break-all text-secondary">{String(item.args?.command || '')}</span></div><pre className="whitespace-pre-wrap break-all font-mono text-[11px] leading-5 text-primary">{item.output || (item.pending ? '正在运行…' : '无输出')}</pre></div>)}
      </div>}
    </div>
    {error && <div className="flex items-center gap-2 border-t border-edge bg-panel px-4 py-2 text-xs text-primary"><span className="flex-1 truncate">{error}</span><button aria-label="关闭错误" onClick={() => setError('')}><X size={13} /></button></div>}
    <section className="work-summary-section">
      <button className="work-summary-heading" onClick={() => setTasksOpen(value => !value)} aria-expanded={tasksOpen}><span><ListTodo size={16} /> TODO <span className="summary-count">{tasks.length}</span></span><ChevronDown size={15} className={tasksOpen ? '' : '-rotate-90'} /></button>
      {tasksOpen && <div className="scrollbar task-list">{tasks.length ? tasks.map(task => <div className="task-row" key={task.task_id}><span className={`task-check ${task.status === 'done' ? 'task-done' : ''}`}>{task.status === 'done' && <Check size={12} />}</span><span className={`min-w-0 flex-1 truncate ${task.status === 'done' ? 'text-subtle line-through' : 'text-secondary'}`} title={task.title}>{task.title}</span><span className="text-[10px] text-subtle">{task.status}</span></div>) : <p className="summary-empty">本会话暂无 TODO。Agent 创建任务后会显示在这里。</p>}</div>}
    </section>
    <section className="work-summary-section">
      <button className="work-summary-heading" onClick={() => setSessionOpen(value => !value)} aria-expanded={sessionOpen}><span><Activity size={16} /> Session</span><ChevronDown size={15} className={sessionOpen ? '' : '-rotate-90'} /></button>
      {sessionOpen && <div className="session-stats">
        <div className="stat-card"><div className="stat-label">Input / Output</div><div className="stat-value">{(state.usage?.input_tokens || 0).toLocaleString()} <span>/ {(state.usage?.output_tokens || 0).toLocaleString()}</span></div><div className="stat-caption">tokens</div></div>
        <div className="stat-card"><div className="stat-label">Context</div><div className="stat-value">{contextPercent}%</div><div className="stat-track"><span style={{ width: `${contextPercent}%` }} /></div></div>
        <div className="stat-card"><div className="stat-label">Rounds</div><div className="stat-value">{state.rounds.toLocaleString()}</div><div className="stat-caption">当前会话</div></div>
        <div className="stat-card"><div className="stat-label">Tool calls</div><div className="stat-value">{toolCount.toLocaleString()}</div><div className="stat-caption">当前会话</div></div>
      </div>}
    </section>
  </aside>
}
