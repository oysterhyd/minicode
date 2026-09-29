import { useMemo, useState } from 'react'
import { ArrowUpRight, FileCode2, FolderOpen, Keyboard, Moon, PanelLeft, PanelRight, Search, Settings2, SquarePen, type LucideIcon } from 'lucide-react'
import { Modal } from './ui/Modal'
import type { Session } from '../types'
import { basename, dirname, fuzzyScore, modKey, relativeTime } from '../lib/format'

type Action = { id: string; group: string; label: string; hint: string; icon: LucideIcon; run: () => void }

export function CommandPalette({ sessions, files, onClose, onNew, onChoose, onSettings, onShortcuts, onLeft, onRight, onTheme, onSession, onFile }: {
  sessions: Session[]; files: string[]; onClose: () => void; onNew: () => void; onChoose: () => void; onSettings: () => void; onShortcuts: () => void
  onLeft: () => void; onRight: () => void; onTheme: () => void; onSession: (session: Session) => void; onFile: (path: string) => void
}) {
  const [query, setQuery] = useState('')
  const [index, setIndex] = useState(0)
  const actions = useMemo(() => {
    const base: Action[] = [
      { id: 'new', group: '操作', label: '新建任务', hint: `${modKey} N`, icon: SquarePen, run: onNew },
      { id: 'choose', group: '操作', label: '打开工作区', hint: '本地目录', icon: FolderOpen, run: onChoose },
      { id: 'settings', group: '操作', label: '打开设置', hint: `${modKey} ,`, icon: Settings2, run: onSettings },
      { id: 'shortcuts', group: '操作', label: '查看键盘快捷键', hint: `${modKey} /`, icon: Keyboard, run: onShortcuts },
      { id: 'left', group: '操作', label: '切换任务侧栏', hint: `${modKey} B`, icon: PanelLeft, run: onLeft },
      { id: 'right', group: '操作', label: '切换工作台', hint: `${modKey} Shift B`, icon: PanelRight, run: onRight },
      { id: 'theme', group: '操作', label: '切换深浅主题', hint: '外观', icon: Moon, run: onTheme },
    ]
    const tasks: Action[] = sessions.map(session => ({ id: session.session_id, group: '任务', label: session.title, hint: `${basename(session.workspace)} · ${relativeTime(session.updated_at || session.created_at)}`, icon: ArrowUpRight, run: () => onSession(session) }))
    if (!query.trim()) return [...base, ...tasks.slice(0, 8)]
    const fileActions: Action[] = files.map(file => ({ id: `file:${file}`, group: '文件', label: basename(file), hint: dirname(file), icon: FileCode2, run: () => onFile(file) }))
    const score = (action: Action) => Math.max(fuzzyScore(action.label, query), fuzzyScore(`${action.label} ${action.hint}`, query) - 100)
    const ranked = (list: Action[], limit: number) => list.map(action => ({ action, value: score(action) })).filter(entry => entry.value >= 0).sort((a, b) => b.value - a.value).slice(0, limit).map(entry => entry.action)
    return [...ranked(base, 7), ...ranked(tasks, 10), ...ranked(fileActions, 8)]
  }, [query, sessions, files])
  const selected = Math.min(index, Math.max(0, actions.length - 1))
  const run = (action: Action) => { onClose(); action.run() }
  return <Modal label="快捷操作" className="command-dialog" onClose={onClose}>
    <div className="command-search"><Search size={17} /><input data-autofocus aria-label="搜索操作和任务" placeholder="搜索操作、任务或文件…" value={query}
      role="combobox" aria-expanded="true" aria-controls="command-results" aria-activedescendant={actions.length ? `command-${selected}` : undefined}
      onChange={event => { setQuery(event.target.value); setIndex(0) }} onKeyDown={event => {
        if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
          event.preventDefault()
          const next = (selected + (event.key === 'ArrowDown' ? 1 : -1) + actions.length) % Math.max(1, actions.length)
          setIndex(next); document.getElementById(`command-${next}`)?.scrollIntoView({ block: 'nearest' })
        }
        if (event.key === 'Enter' && actions[selected]) { event.preventDefault(); run(actions[selected]) }
      }} /><kbd className="kbd">Esc</kbd></div>
    <div className="command-results scrollbar" id="command-results" role="listbox" aria-label="操作结果">
      {actions.map((action, i) => <div key={action.id} role="presentation">
        {(i === 0 || actions[i - 1].group !== action.group) && <div className="command-group">{action.group}</div>}
        <button id={`command-${i}`} role="option" aria-selected={selected === i} className={`command-option ${selected === i ? 'is-selected' : ''}`}
          onMouseMove={() => setIndex(i)} onClick={() => run(action)}>
          <action.icon size={15} /><span className="truncate">{action.label}</span><small className="truncate">{action.hint}</small>
        </button>
      </div>)}
      {!actions.length && <p className="command-empty">没有匹配“{query}”的结果</p>}
    </div>
    <footer className="command-footer"><span><kbd>↑</kbd><kbd>↓</kbd> 选择</span><span><kbd>Enter</kbd> 打开</span><span><kbd>Esc</kbd> 关闭</span></footer>
  </Modal>
}
