import { useState } from 'react'
import { AnimatePresence, motion } from 'motion/react'
import { Check, ChevronRight, FolderOpen, LoaderCircle, MessageSquare, Moon, PanelLeftClose, PanelLeftOpen, Plus, Search, Settings2, ShieldAlert, Sun } from 'lucide-react'
import type { Session } from '../types'

type Props = {
  workspace: string | null; sessions: Session[]; activeSession: string | null; collapsed: boolean; theme: 'light' | 'dark'
  runningSessions: { id: string; approval: boolean }[]; unreadSessions: string[]
  onToggle: () => void; onTheme: () => void; onSearch: () => void; onSettings: () => void; onChoose: () => void; onNew: () => void
  onWorkspace: (workspace: string) => void; onSession: (session: Session) => void
}

function relativeDate(value: string) {
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return ''
  if (date.toDateString() === new Date().toDateString()) return '今天'
  return `${date.getMonth() + 1}/${date.getDate()}`
}

export function Sidebar(props: Props) {
  const [query, setQuery] = useState('')
  const [closed, setClosed] = useState<string[]>([])
  const groups = Array.from(new Set([...(props.workspace ? [props.workspace] : []), ...props.sessions.map(s => s.workspace)]))
  const ThemeIcon = props.theme === 'dark' ? Sun : Moon
  if (props.collapsed) return <aside className="rail sidebar-rail" aria-label="已收拢的侧栏">
    <img className="rail-brand" src="./app-mark.svg" alt="MiniCode" />
    <button className="rail-button" onClick={props.onToggle} title="展开侧栏 · Ctrl B" aria-label="展开侧栏"><PanelLeftOpen size={18} /></button>
    <button className="rail-button" onClick={props.onNew} title="新建任务" aria-label="新建任务"><Plus size={18} /></button>
    <button className="rail-button" onClick={props.onSearch} title="快捷操作 · Ctrl K" aria-label="快捷操作"><Search size={17} /></button>
    <div className="rail-bottom"><button className="rail-button" onClick={props.onTheme} aria-label="切换主题"><ThemeIcon size={17} /></button><button className="rail-button" onClick={props.onSettings} title="设置" aria-label="设置"><Settings2 size={17} /></button></div>
  </aside>
  return <aside className="sidebar">
    <header className="sidebar-brand"><img className="brand-mark" src="./app-mark.svg" alt="" /><span>MiniCode</span><button className="icon-button" onClick={props.onToggle} title="收拢侧栏 · Ctrl B" aria-label="收拢侧栏"><PanelLeftClose size={17} /></button></header>
    <div className="sidebar-actions"><button className="new-task-button" onClick={props.onNew}><Plus size={17} />新建任务<kbd>Ctrl N</kbd></button>
      <button className="nav-row" onClick={props.onSearch}><Search size={16} />快捷操作<kbd>Ctrl K</kbd></button></div>
    <div className="sidebar-section-heading"><span>工作空间</span><button className="icon-button" onClick={props.onChoose} title="打开本地目录" aria-label="打开本地目录"><Plus size={15} /></button></div>
    <div className="scrollbar sidebar-projects">
      {!groups.length && <button className="workspace-empty" onClick={props.onChoose}><FolderOpen size={23} /><strong>打开你的项目</strong><span>选择本地目录，开始第一个任务</span></button>}
      {groups.map(group => {
        const current = group === props.workspace
        const projectSessions = props.sessions.filter(s => s.workspace === group && s.title.toLowerCase().includes(query.toLowerCase()))
        const expanded = !closed.includes(group)
        return <section key={group} className="project-group">
          <div className={`project-heading ${current ? 'project-current' : ''}`}>
            <button className="project-disclosure" aria-label={`${expanded ? '收起' : '展开'} ${group.split(/[\\/]/).pop()}`} aria-expanded={expanded} onClick={() => setClosed(value => expanded ? [...value, group] : value.filter(item => item !== group))}><ChevronRight size={13} className={expanded ? 'rotate-90' : ''} /></button>
            <button className="project-row" onClick={() => { if (!current) props.onWorkspace(group) }} title={group}><FolderOpen size={16} /><span>{group.split(/[\\/]/).pop()}</span>{current && <span className="project-current-label">当前</span>}</button>
          </div>
          <AnimatePresence initial={false}>{expanded && <motion.div className="project-sessions" initial={{ opacity: 0, y: -4 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: -4 }} transition={{ duration: .15 }}>
            {current && props.sessions.length > 4 && <label className="session-search"><Search size={13} /><input aria-label="筛选任务" placeholder="筛选任务…" value={query} onChange={event => setQuery(event.target.value)} /></label>}
            {projectSessions.length === 0 && <p className="sidebar-empty">{query ? '没有匹配的任务' : '新任务会保存在这里'}</p>}
            {projectSessions.map(session => <button key={session.session_id} onClick={() => props.onSession(session)} aria-current={props.activeSession === session.session_id ? 'page' : undefined}
              className={`session-row ${props.activeSession === session.session_id ? 'session-active' : ''}`} title={session.title}>
              {props.activeSession === session.session_id && <motion.span className="session-selection" layoutId="session-selection" transition={{ type: 'spring', stiffness: 450, damping: 36 }} />}
              {props.runningSessions.find(run => run.id === session.session_id)?.approval ? <ShieldAlert size={13} className="text-accent" aria-label="等待审批" /> : props.runningSessions.some(run => run.id === session.session_id) ? <LoaderCircle size={13} className="animate-spin text-accent" aria-label="运行中" /> : props.unreadSessions.includes(session.session_id) ? <Check size={13} className="text-accent" aria-label="有新结果" /> : session.status === 'completed' ? <Check size={13} /> : <MessageSquare size={13} />}<span className="session-title">{session.title}</span><time>{relativeDate(session.created_at)}</time>
            </button>)}
          </motion.div>}</AnimatePresence>
        </section>
      })}
    </div>
    <footer className="sidebar-footer"><button className="nav-row" onClick={props.onSettings}><Settings2 size={16} />设置<kbd>Ctrl ,</kbd></button><button className="icon-button theme-toggle" onClick={props.onTheme} aria-label="切换主题" title={props.theme === 'dark' ? '切换为浅色' : '切换为深色'}><ThemeIcon size={16} /></button></footer>
  </aside>
}
