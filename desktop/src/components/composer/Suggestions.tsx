import { motion } from 'motion/react'
import { AtSign, CornerDownLeft, FileCode2, Slash } from 'lucide-react'
import type { Capabilities, Command, Model, Session } from '../../types'
import { basename, dirname, pathScore } from '../../lib/format'

export type Suggestion = { value: string; detail: string; kind: 'command' | 'value' | 'file' }
const SUBMENUS = ['/model', '/effort', '/permissions', '/resume', '/skill']

export function slashSuggestions(draft: string, { commands, models, sessions, capabilities }: {
  commands: Command[]; models: Model[]; sessions: Session[]; capabilities: Capabilities | null
}): Suggestion[] {
  if (!draft.startsWith('/') || draft.includes('\n')) return []
  const verb = draft.split(/\s/, 1)[0].toLowerCase()
  const hasSpace = draft.includes(' ')
  const query = hasSpace ? draft.slice(draft.indexOf(' ') + 1).toLowerCase() : ''
  if (SUBMENUS.includes(verb) && (hasSpace || draft === verb)) {
    const values = verb === '/model' ? models.map(item => ({ value: item.id, detail: item.available ? item.provider : '未配置' })) :
      verb === '/effort' ? ['off', 'low', 'medium', 'high', 'xhigh', 'max'].map(value => ({ value, detail: '思考强度' })) :
      verb === '/permissions' ? [['default', '逐项确认'], ['accept_edits', '自动编辑'], ['bypass', '全部允许']].map(([value, detail]) => ({ value, detail })) :
      verb === '/resume' ? sessions.slice(0, 30).map(item => ({ value: item.session_id.slice(0, 8), detail: item.title })) :
      capabilities?.skills.map(item => ({ value: item.name, detail: item.description })) || []
    return values.filter(item => `${item.value} ${item.detail}`.toLowerCase().includes(query)).slice(0, 8).map(item => ({ ...item, kind: 'value' }))
  }
  if (hasSpace) return []
  return commands.filter(command => command.name.startsWith(verb)).slice(0, 10).map(command => ({ value: command.name, detail: command.summary, kind: 'command' }))
}

export function isSubmenu(value: string) { return SUBMENUS.includes(value) }

export function fileSuggestions(query: string, files: string[]): Suggestion[] {
  return files.map(file => ({ file, score: pathScore(file, query) }))
    .filter(entry => entry.score >= 0 || !query).sort((a, b) => b.score - a.score || a.file.length - b.file.length)
    .slice(0, 8).map(entry => ({ value: entry.file, detail: dirname(entry.file), kind: 'file' }))
}

export function SuggestionList({ id, label, hint, items, active, onPick, onHover }: {
  id: string; label: string; hint: string; items: Suggestion[]; active: number; onPick: (item: Suggestion) => void; onHover: (index: number) => void
}) {
  return <motion.div className="suggestions" id={id} role="listbox" aria-label={label}
    initial={{ opacity: 0, y: 6, scale: .99 }} animate={{ opacity: 1, y: 0, scale: 1 }} exit={{ opacity: 0, y: 4 }} transition={{ duration: .13, ease: [.2, .8, .2, 1] }}>
    <div className="suggestions-heading"><span>{label}</span><small>{hint}</small></div>
    {items.map((item, index) => <button key={item.value} id={`${id}-${index}`} role="option" aria-selected={index === active}
      className={`suggestion ${index === active ? 'is-active' : ''}`} onMouseDown={event => event.preventDefault()} onMouseMove={() => onHover(index)} onClick={() => onPick(item)}>
      {item.kind === 'file' ? <FileCode2 size={14} /> : item.kind === 'command' ? <Slash size={13} /> : <AtSign size={13} />}
      <span className={item.kind === 'file' ? 'suggestion-file' : 'suggestion-command'}>{item.kind === 'file' ? basename(item.value) : item.value}</span>
      <span className="suggestion-detail">{item.detail}</span>
      {index === active && <CornerDownLeft size={12} className="suggestion-enter" />}
    </button>)}
  </motion.div>
}
