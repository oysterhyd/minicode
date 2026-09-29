import { useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { AnimatePresence, motion, useReducedMotion } from 'motion/react'
import { ArrowDown } from 'lucide-react'
import type { FeedItem } from '../../types'
import { ApprovalCard } from './ApprovalCard'
import { AssistantMessage, Notice, Thinking, ToolGroup, TurnSummary, UserMessage } from './Messages'
import { ToolStep } from './ToolStep'

type Row = { type: 'item'; item: FeedItem } | { type: 'tools'; id: string; items: FeedItem[] }

/** Collapse consecutive tool calls into one group so long runs stay readable. */
function rows(items: FeedItem[]): Row[] {
  const result: Row[] = []
  for (const item of items) {
    const last = result[result.length - 1]
    if (item.kind === 'tool') {
      if (last?.type === 'tools') last.items.push(item)
      else result.push({ type: 'tools', id: `tools-${item.id}`, items: [item] })
    } else result.push({ type: 'item', item })
  }
  return result
}

export function SessionFeed({ items, busy, sessionId, clientKey, expandTools, onDecide, onEdit, onRetry, footer }: {
  items: FeedItem[]; busy: boolean; sessionId: string | null; clientKey: string; expandTools: boolean; footer?: ReactNode
  onDecide: (id: string, granted: boolean, remember?: boolean) => Promise<void>; onEdit: (text: string) => void; onRetry: (text: string) => void
}) {
  const viewport = useRef<HTMLDivElement>(null)
  const follow = useRef(true)
  const [showLatest, setShowLatest] = useState(false)
  const reducedMotion = useReducedMotion()
  const grouped = useMemo(() => rows(items), [items])
  const lastUser = items.findLastIndex(item => item.kind === 'user')
  const lastAssistant = items.findLastIndex(item => item.kind === 'assistant')
  const lastUserText = lastUser >= 0 ? items[lastUser].text || '' : ''
  useLayoutEffect(() => {
    if (follow.current && viewport.current) viewport.current.scrollTop = viewport.current.scrollHeight
  }, [items, busy])
  useEffect(() => {
    const scroll = viewport.current
    const content = scroll?.firstElementChild
    if (!scroll || !content) return
    const observer = new ResizeObserver(() => { if (follow.current) scroll.scrollTop = scroll.scrollHeight })
    observer.observe(content)
    return () => observer.disconnect()
  }, [])
  const hasLive = items.some(item => item.pending || item.approvalId || item.kind === 'thinking')
  // The runtime announces a tool before asking for approval; show that row as waiting, not running.
  const approval = items.find(item => item.approvalId)
  const waitingId = approval ? items.findLast(item => item.kind === 'tool' && item.pending && item.name === approval.name)?.id : undefined
  return <div className="feed-container">
    <div ref={viewport} className="session-feed scrollbar" onScroll={() => {
      const element = viewport.current!
      follow.current = element.scrollHeight - element.scrollTop - element.clientHeight < 80
      setShowLatest(!follow.current)
    }}>
      <div className="feed-content" role="log" aria-label="对话记录" aria-busy={busy}>
        {grouped.map((row, index) => {
          if (row.type === 'tools') {
            const live = busy && index === grouped.length - 1
            if (row.items.length > 1) return <ToolGroup key={row.id} items={row.items} sessionId={sessionId} clientKey={clientKey} live={live} expandTools={expandTools} waitingId={waitingId} />
            const [only] = row.items
            // A running shell command opens so its output streams in view.
            return <ToolStep key={row.id} item={only} sessionId={sessionId} clientKey={clientKey} waiting={only.id === waitingId}
              defaultOpen={expandTools || (only.pending && only.name === 'bash' && only.id !== waitingId)} />
          }
          const item = row.item
          const position = items.indexOf(item)
          if (item.kind === 'approval') return <ApprovalCard key={item.id} item={item} onDecide={onDecide} />
          if (item.kind === 'notice') return <Notice key={item.id} item={item} />
          if (item.kind === 'summary') return <TurnSummary key={item.id} item={item} />
          if (item.kind === 'thinking') return busy && !items.some(other => other.pending || other.approvalId) ? <Thinking key={item.id} item={item} /> : null
          if (item.kind === 'user') return <UserMessage key={item.id} item={item} onEdit={!busy && position === lastUser ? onEdit : undefined} />
          return <AssistantMessage key={item.id} item={item} onRetry={!busy && position === lastAssistant && lastUser >= 0 && lastUser < position ? () => onRetry(lastUserText) : undefined} />
        })}
        {busy && !hasLive && <Thinking item={{ id: 'busy', kind: 'thinking', text: '任务处理中' }} />}
        {footer}
      </div>
    </div>
    <AnimatePresence>{showLatest && <motion.button className="jump-latest" initial={{ opacity: 0, y: 10, scale: .95 }} animate={{ opacity: 1, y: 0, scale: 1 }} exit={{ opacity: 0, y: 10, scale: .95 }}
      onClick={() => {
        follow.current = true; setShowLatest(false)
        viewport.current?.scrollTo({ top: viewport.current.scrollHeight, behavior: reducedMotion ? 'instant' : 'smooth' })
      }}><ArrowDown size={14} />回到最新</motion.button>}</AnimatePresence>
  </div>
}
