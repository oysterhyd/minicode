import { useState } from 'react'
import { ArrowUpRight, FolderOpen, Moon, PanelLeft, PanelRight, Search, Settings2, SquarePen } from 'lucide-react'
import { Modal } from './Modal'
import type { Session } from '../types'

export function CommandPalette({ sessions, onClose, onNew, onChoose, onSettings, onLeft, onRight, onTheme, onSession, busy }: {
  sessions: Session[]; onClose: () => void; onNew: () => void; onChoose: () => void; onSettings: () => void;
  onLeft: () => void; onRight: () => void; onTheme: () => void; onSession: (session: Session) => void; busy: boolean
}) {
  const [query, setQuery] = useState('')
  const [index, setIndex] = useState(0)
  const actions = [
    { id: 'new', label: '新建任务', hint: 'Ctrl N', icon: SquarePen, run: onNew },
    { id: 'choose', label: '打开工作区', hint: '本地目录', icon: FolderOpen, run: onChoose },
    { id: 'settings', label: '打开设置', hint: 'Ctrl ,', icon: Settings2, run: onSettings },
    { id: 'left', label: '切换任务侧栏', hint: 'Ctrl B', icon: PanelLeft, run: onLeft },
    { id: 'right', label: '切换工作台', hint: 'Ctrl Shift B', icon: PanelRight, run: onRight },
    { id: 'theme', label: '切换深浅主题', hint: '外观', icon: Moon, run: onTheme },
    ...sessions.map(session => ({ id: session.session_id, label: session.title, hint: session.workspace.split(/[\\/]/).pop() || '', icon: ArrowUpRight, run: () => onSession(session) })),
  ].filter(action => !('disabled' in action && action.disabled) && `${action.label} ${action.hint}`.toLowerCase().includes(query.toLowerCase()))
  const selected = Math.min(index, Math.max(0, actions.length - 1))
  const run = (action: typeof actions[number]) => { onClose(); action.run() }
  return <Modal label="快捷操作" className="command-dialog" onClose={onClose}>
    <div className="command-search"><Search size={20} /><input data-autofocus aria-label="搜索操作和任务" placeholder="搜索操作或跳转到任务…" value={query}
      role="combobox" aria-expanded="true" aria-controls="command-results" aria-activedescendant={actions.length ? `command-${selected}` : undefined}
      onChange={event => { setQuery(event.target.value); setIndex(0) }} onKeyDown={event => {
        if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
          event.preventDefault()
          const next = (selected + (event.key === 'ArrowDown' ? 1 : -1) + actions.length) % Math.max(1, actions.length)
          setIndex(next); document.getElementById(`command-${next}`)?.scrollIntoView({ block: 'nearest' })
        }
        if (event.key === 'Enter' && actions[selected]) { event.preventDefault(); run(actions[selected]) }
      }} /><button className="keycap" onClick={onClose} aria-label="关闭快捷操作">Esc</button></div>
    <div className="command-results scrollbar" id="command-results" role="listbox" aria-label="操作结果">
      <div className="section-label">{query ? '搜索结果' : '常用操作与最近任务'}</div>
      {actions.map((action, i) => <button key={action.id} id={`command-${i}`} role="option" aria-selected={selected === i}
        className={`command-option ${selected === i ? 'command-selected' : ''}`} onMouseMove={() => setIndex(i)} onClick={() => run(action)}>
        <action.icon size={17} /><span>{action.label}</span><small>{action.hint}</small>
      </button>)}
      {!actions.length && <p className="panel-empty">没有匹配的操作或任务</p>}
    </div>
    <footer className="command-footer"><span><kbd>↑</kbd><kbd>↓</kbd> 选择</span><span><kbd>Enter</kbd> 打开</span></footer>
  </Modal>
}
