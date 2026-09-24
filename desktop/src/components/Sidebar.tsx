import { ChevronDown, ChevronRight, FolderOpen, PanelLeftClose, PanelLeftOpen, Plus, Settings2 } from 'lucide-react'
import type { Session } from '../types'

type Props = {
  workspace: string | null
  sessions: Session[]
  activeSession: string | null
  collapsed: boolean
  onToggle: () => void
  onSettings: () => void
  onChoose: () => void
  onNew: () => void
  onWorkspace: (workspace: string) => void
  onSession: (session: Session) => void
}

export function Sidebar(props: Props) {
  const groups = Array.from(new Set([...(props.workspace ? [props.workspace] : []), ...props.sessions.map(s => s.workspace)]))
  if (props.collapsed) return <aside className="rail border-r border-edge bg-sidebar" aria-label="已收拢的侧栏">
    <button className="rail-button mt-4" onClick={props.onToggle} title="展开侧栏" aria-label="展开侧栏"><PanelLeftOpen size={19} /></button>
    <button className="rail-button mt-5" onClick={props.onNew} title="新建任务" aria-label="新建任务"><Plus size={19} /></button>
    <button className="rail-button mt-2" onClick={props.onChoose} title="选择本地目录" aria-label="选择本地目录"><FolderOpen size={18} /></button>
    <button className="rail-button mt-auto mb-4" onClick={() => { props.onToggle(); props.onSettings() }} title="设置" aria-label="设置"><Settings2 size={18} /></button>
  </aside>
  return <aside className="sidebar flex h-full min-w-0 flex-col border-r border-edge bg-sidebar">
    <div className="flex h-16 items-center justify-between px-5">
      <div className="flex min-w-0 items-center gap-3 text-[15px] font-semibold tracking-tight text-primary"><img className="brand-mark" src="./app-mark.svg" alt="" /><span className="truncate">MiniCode Desktop</span></div>
      <button className="icon-button shrink-0" onClick={props.onToggle} title="收拢侧栏" aria-label="收拢侧栏"><PanelLeftClose size={18} /></button>
    </div>
    <div className="px-3 pb-4">
      <button className="primary-soft w-full" onClick={props.onNew}><Plus size={17} /> 新建任务 <span className="ml-auto text-xs opacity-60">Ctrl N</span></button>
    </div>
    <div className="scrollbar min-h-0 flex-1 overflow-y-auto px-3">
      <div className="section-label mb-2 px-2">工作区</div>
      <button className="nav-row mb-3 text-secondary" onClick={props.onChoose}><FolderOpen size={16} /> 选择本地目录</button>
      {groups.map(group => {
        const current = group === props.workspace
        const projectSessions = props.sessions.filter(s => s.workspace === group)
        return <div key={group} className="mb-3">
          <button className={`project-row ${current ? 'text-white' : 'text-secondary'}`} onClick={() => props.onWorkspace(group)} title={group}>
            {current ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
            <FolderOpen size={16} />
            <span className="truncate font-medium">{group.split(/[\\/]/).pop()}</span>
            {current && <span className="ml-auto h-1.5 w-1.5 rounded-full bg-white/80" />}
          </button>
          {current && <div className="mt-1 space-y-0.5 pl-6">
            {projectSessions.length === 0 && <div className="px-3 py-2 text-xs text-subtle">暂无会话</div>}
            {projectSessions.map(session => <button key={session.session_id} onClick={() => props.onSession(session)}
              className={`session-row ${props.activeSession === session.session_id ? 'session-active' : ''}`} title={session.title}>
              <span className={`status-dot ${session.status === 'completed' ? 'bg-white/70' : 'bg-white/35'}`} />
              <span className="truncate">{session.title}</span>
            </button>)}
          </div>}
        </div>
      })}
    </div>
    <div className="border-t border-edge p-3">
      <button className="nav-row" onClick={props.onSettings}><Settings2 size={17} /> Settings <ChevronRight size={14} className="ml-auto text-subtle" /></button>
    </div>
  </aside>
}
