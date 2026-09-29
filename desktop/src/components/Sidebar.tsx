import { useEffect, useMemo, useRef, useState } from 'react'
import { AnimatePresence, motion } from 'motion/react'
import { ChevronRight, Ellipsis, FolderOpen, FolderPlus, LoaderCircle, Moon, PanelLeftOpen, Pencil, Pin, PinOff, Plus, Search, Settings2, ShieldAlert, SquarePen, Sun, SunMoon, Trash2, X } from 'lucide-react'
import type { Session } from '../types'
import { basename, fuzzyScore, modKey, relativeTime } from '../lib/format'
import type { ThemePreference } from '../lib/prefs'
import { Menu, type MenuItem } from './ui/Menu'

type Props = {
  workspace: string | null; sessions: Session[]; activeSession: string | null; collapsed: boolean; theme: ThemePreference
  running: Record<string, { approval: boolean }>; unread: string[]
  onToggle: () => void; onTheme: () => void; onSettings: () => void; onChoose: () => void; onNew: () => void
  onWorkspace: (workspace: string) => void; onSession: (session: Session) => void
  onRename: (session: Session, title: string) => void; onPin: (session: Session) => void; onDelete: (session: Session) => void
}

const PAGE = 8
const themeIcon = { system: SunMoon, light: Sun, dark: Moon }
const themeLabel = { system: '跟随系统', light: '浅色', dark: '深色' }

function SessionRow({ session, active, status, unread, onOpen, onMenu, renaming, onRename, onCancelRename, onStartRename }: {
  session: Session; active: boolean; status?: { approval: boolean }; unread: boolean; renaming: boolean
  onOpen: () => void; onStartRename: () => void; onMenu: (anchor: HTMLElement | { x: number; y: number }) => void; onRename: (title: string) => void; onCancelRename: () => void
}) {
  const [value, setValue] = useState(session.title)
  const input = useRef<HTMLInputElement>(null)
  useEffect(() => { if (renaming) { setValue(session.title); requestAnimationFrame(() => input.current?.select()) } }, [renaming])
  const icon = status?.approval ? <ShieldAlert size={13} className="session-icon is-approval" aria-label="等待审批" />
    : status ? <LoaderCircle size={13} className="session-icon is-running spin" aria-label="运行中" />
    : unread ? <span className="session-unread" aria-label="有新结果" /> : session.pinned ? <Pin size={12} className="session-icon" /> : null
  if (renaming) return <div className="session-row is-renaming">
    <input ref={input} aria-label="重命名任务" value={value} maxLength={120} onChange={event => setValue(event.target.value)}
      onKeyDown={event => {
        if (event.key === 'Enter') { event.preventDefault(); onRename(value.trim()) }
        if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); onCancelRename() }
      }} onBlur={() => onRename(value.trim())} />
  </div>
  return <div className={`session-row ${active ? 'is-active' : ''}`} onContextMenu={event => { event.preventDefault(); onMenu({ x: event.clientX, y: event.clientY }) }}>
    {active && <motion.span className="session-selection" layoutId="session-selection" transition={{ type: 'spring', stiffness: 500, damping: 40 }} />}
    <button className="session-main" onClick={onOpen} onDoubleClick={onStartRename} aria-current={active ? 'page' : undefined} title={session.title}>
      {icon}<span className="session-title">{session.title}</span>
      <time>{relativeTime(session.updated_at || session.created_at)}</time>
    </button>
    <button className="session-more" aria-label={`${session.title} 更多操作`} onClick={event => onMenu(event.currentTarget)}><Ellipsis size={14} /></button>
  </div>
}
export function Sidebar(props: Props) {
  const [query, setQuery] = useState('')
  const [closed, setClosed] = useState<string[]>([])
  const [limits, setLimits] = useState<Record<string, number>>({})
  const [renaming, setRenaming] = useState<string | null>(null)
  const [menu, setMenu] = useState<{ session: Session; anchor: HTMLElement | { x: number; y: number } } | null>(null)
  const search = useRef<HTMLInputElement>(null)
  const ThemeIcon = themeIcon[props.theme]
  useEffect(() => {
    const focus = () => { search.current?.focus(); search.current?.select() }
    window.addEventListener('minicode:focus-search', focus)
    return () => window.removeEventListener('minicode:focus-search', focus)
  }, [])

  const sorted = useMemo(() => [...props.sessions].sort((a, b) => (b.updated_at || b.created_at).localeCompare(a.updated_at || a.created_at)), [props.sessions])
  const matches = query.trim() ? sorted.map(session => ({ session, score: Math.max(fuzzyScore(session.title, query), fuzzyScore(basename(session.workspace), query) - 200) }))
    .filter(entry => entry.score >= 0).sort((a, b) => b.score - a.score).map(entry => entry.session) : null
  const pinned = sorted.filter(session => session.pinned)
  const groups = Array.from(new Set([...(props.workspace ? [props.workspace] : []), ...sorted.map(session => session.workspace)]))

  const row = (session: Session) => <SessionRow key={session.session_id} session={session} active={props.activeSession === session.session_id}
    status={props.running[session.session_id]} unread={props.unread.includes(session.session_id)} renaming={renaming === session.session_id}
    onOpen={() => props.onSession(session)} onMenu={anchor => setMenu({ session, anchor })} onStartRename={() => setRenaming(session.session_id)}
    onRename={title => { setRenaming(null); if (title && title !== session.title) props.onRename(session, title) }} onCancelRename={() => setRenaming(null)} />

  const menuItems = (session: Session): MenuItem[] => [
    { label: '打开', icon: ChevronRight, run: () => props.onSession(session) },
    { label: '重命名', icon: Pencil, run: () => setRenaming(session.session_id) },
    { label: session.pinned ? '取消置顶' : '置顶', icon: session.pinned ? PinOff : Pin, run: () => props.onPin(session) },
    'divider',
    { label: '删除任务', icon: Trash2, danger: true, disabled: Boolean(props.running[session.session_id]), run: () => props.onDelete(session) },
  ]

  if (props.collapsed) return <aside className="rail sidebar-rail" aria-label="已收拢的侧栏">
    <button className="rail-button" onClick={props.onToggle} aria-label="展开侧栏" data-tip="展开侧栏" data-shortcut={`${modKey} B`}><PanelLeftOpen size={17} /></button>
    <button className="rail-button" onClick={props.onNew} aria-label="新建任务" data-tip="新建任务" data-shortcut={`${modKey} N`}><SquarePen size={17} /></button>
    <button className="rail-button" onClick={() => { props.onToggle(); setTimeout(() => search.current?.focus(), 250) }} aria-label="搜索任务" data-tip="搜索任务"><Search size={17} /></button>
    <div className="rail-bottom">
      <button className="rail-button" onClick={props.onTheme} aria-label="切换主题" data-tip={`主题：${themeLabel[props.theme]}`}><ThemeIcon size={16} /></button>
      <button className="rail-button" onClick={props.onSettings} aria-label="设置" data-tip="设置" data-shortcut={`${modKey} ,`}><Settings2 size={16} /></button>
    </div>
  </aside>

  return <aside className="sidebar" aria-label="任务侧栏">
    <div className="sidebar-top">
      <button className="new-task-button" onClick={props.onNew}><SquarePen size={15} /><span>新建任务</span><kbd>{modKey} N</kbd></button>
      <label className="sidebar-search"><Search size={13} />
        <input ref={search} aria-label="搜索任务" placeholder="搜索任务" value={query} onChange={event => setQuery(event.target.value)}
          onKeyDown={event => { if (event.key === 'Escape' && query) { event.stopPropagation(); setQuery('') } }} />
        {query && <button aria-label="清空搜索" onClick={() => setQuery('')}><X size={12} /></button>}
      </label>
    </div>
    <div className="sidebar-scroll scrollbar">
      {matches ? <section className="sidebar-section">
        <div className="sidebar-heading"><span>搜索结果 · {matches.length}</span></div>
        {matches.length ? matches.slice(0, 50).map(row) : <p className="sidebar-empty">没有匹配“{query}”的任务</p>}
      </section> : <>
        {pinned.length > 0 && <section className="sidebar-section"><div className="sidebar-heading"><span>置顶</span></div>{pinned.map(row)}</section>}
        <div className="sidebar-heading sidebar-heading-workspaces"><span>工作空间</span>
          <button className="icon-button icon-button-small" onClick={props.onChoose} aria-label="打开本地目录" data-tip="打开本地目录"><FolderPlus size={14} /></button></div>
        {!groups.length && <button className="workspace-empty" onClick={props.onChoose}><FolderOpen size={20} /><strong>打开你的项目</strong><span>选择本地目录，开始第一个任务</span></button>}
        {groups.map(group => {
          const current = group === props.workspace
          const items = sorted.filter(session => session.workspace === group && !session.pinned)
          const expanded = !closed.includes(group)
          const limit = limits[group] || PAGE
          return <section key={group} className="project-group">
            <div className={`project-heading ${current ? 'is-current' : ''}`}>
              <button className="project-toggle" aria-label={`${expanded ? '收起' : '展开'} ${basename(group)}`} aria-expanded={expanded}
                onClick={() => setClosed(value => expanded ? [...value, group] : value.filter(item => item !== group))}>
                <ChevronRight size={12} className={`project-chevron ${expanded ? 'is-open' : ''}`} />
              </button>
              <button className="project-row" onClick={() => { if (!current) props.onWorkspace(group) }} data-tip={group} data-tip-placement="bottom">
                <FolderOpen size={14} /><span>{basename(group)}</span>{current && <span className="project-current">当前</span>}
              </button>
              {current && <button className="icon-button icon-button-small project-add" aria-label={`在 ${basename(group)} 中新建任务`} data-tip="新建任务" onClick={props.onNew}><Plus size={13} /></button>}
            </div>
            <AnimatePresence initial={false}>{expanded && <motion.div className="project-sessions" initial={{ height: 0, opacity: 0 }} animate={{ height: 'auto', opacity: 1 }} exit={{ height: 0, opacity: 0 }}
              transition={{ height: { type: 'spring', stiffness: 500, damping: 42 }, opacity: { duration: .14 } }}>
              {!items.length && <p className="sidebar-empty">新任务会保存在这里</p>}
              {items.slice(0, limit).map(row)}
              {items.length > limit && <button className="sidebar-more" onClick={() => setLimits(value => ({ ...value, [group]: limit + 20 }))}>显示更多（{items.length - limit}）</button>}
            </motion.div>}</AnimatePresence>
          </section>
        })}
      </>}
    </div>
    <footer className="sidebar-footer">
      <button className="nav-row" onClick={props.onSettings} aria-label={`设置 ${modKey} ,`}><Settings2 size={15} /><span>设置</span><kbd>{modKey} ,</kbd></button>
      <button className="icon-button" onClick={props.onTheme} aria-label="切换主题" data-tip={`主题：${themeLabel[props.theme]}`}><ThemeIcon size={15} /></button>
    </footer>
    {menu && <Menu label="任务操作" anchor={menu.anchor} items={menuItems(menu.session)} onClose={() => setMenu(null)} />}
  </aside>
}
