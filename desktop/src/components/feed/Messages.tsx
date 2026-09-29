import { lazy, memo, Suspense, useState } from 'react'
import { AnimatePresence, motion } from 'motion/react'
import { CircleAlert, CircleCheck, CircleSlash, Info, Layers, PencilLine, RotateCcw, TriangleAlert } from 'lucide-react'
import type { FeedItem } from '../../types'
import { summarizeTools } from '../../lib/tools'
import { duration } from '../../lib/format'
import { useNow } from '../../hooks/useNow'
import { CopyButton } from '../CopyButton'
import { ToolStep, reveal } from './ToolStep'

const MessageContent = lazy(() => import('../MessageContent'))
export const enter = { initial: { opacity: 0, y: 6 }, animate: { opacity: 1, y: 0 }, transition: { duration: .22, ease: [.2, .8, .2, 1] as const } }

export const UserMessage = memo(function UserMessage({ item, onEdit }: { item: FeedItem; onEdit?: (text: string) => void }) {
  return <motion.div className="user-turn" {...enter}>
    <div className="user-bubble">{item.text}</div>
    <div className="message-actions message-actions-end">
      <CopyButton text={item.text || ''} label="复制消息" iconOnly />
      {onEdit && <button className="icon-button icon-button-small" aria-label="编辑并重新发送" data-tip="编辑并重新发送" onClick={() => onEdit(item.text || '')}><PencilLine size={13} /></button>}
    </div>
  </motion.div>
})

export const AssistantMessage = memo(function AssistantMessage({ item, onRetry }: { item: FeedItem; onRetry?: () => void }) {
  return <motion.div className="assistant-turn" {...enter}>
    <div className={`message-markdown ${item.pending ? 'is-streaming' : ''}`}>
      <Suspense fallback={<div className="plain-text">{item.text}</div>}><MessageContent text={item.text || ''} streaming={item.pending} /></Suspense>
    </div>
    {!item.pending && <div className="message-actions">
      <CopyButton text={item.text || ''} label="复制回复" iconOnly />
      {onRetry && <button className="icon-button icon-button-small" aria-label="重新生成" data-tip="重新发送上一条消息" onClick={onRetry}><RotateCcw size={13} /></button>}
    </div>}
  </motion.div>
})

export function Thinking({ item }: { item: FeedItem }) {
  const now = useNow(true)
  const seconds = item.startedAt ? Math.floor((now - item.startedAt) / 1000) : 0
  return <motion.div className="thinking-row" role="status" initial={{ opacity: 0 }} animate={{ opacity: 1 }} transition={{ delay: .12 }}>
    <span className="thinking-orb" aria-hidden="true" /><span className="shimmer-text">{item.text || '正在思考'}</span>{seconds >= 2 && <time>{seconds}s</time>}
  </motion.div>
}

export function Notice({ item }: { item: FeedItem }) {
  const Icon = item.tone === 'warning' ? TriangleAlert : item.tone === 'error' ? CircleAlert : Info
  return <motion.div className={`system-note note-${item.tone || 'info'}`} {...enter}><Icon size={14} /><span>{item.text}</span></motion.div>
}

export function TurnSummary({ item }: { item: FeedItem }) {
  const tools = Number(item.text || 0)
  const Icon = item.interrupted ? CircleSlash : item.success ? CircleCheck : TriangleAlert
  return <motion.div className={`turn-summary ${item.success ? '' : 'is-warning'}`} {...enter}>
    <span className="turn-summary-line" /><Icon size={13} />
    <span>{item.interrupted ? '已停止' : item.success ? '已完成' : '已结束'} · 用时 {duration((item.endedAt || 0) - (item.startedAt || 0))}{tools ? ` · ${tools} 个操作` : ''}</span>
    <span className="turn-summary-line" />
  </motion.div>
}

export function ToolGroup({ items, sessionId, clientKey, live, expandTools, waitingId }: { items: FeedItem[]; sessionId: string | null; clientKey: string; live: boolean; expandTools: boolean; waitingId?: string }) {
  const [open, setOpen] = useState<boolean | null>(null)
  const running = items.some(item => item.pending && item.id !== waitingId)
  const failed = items.filter(item => item.success === false && !item.pending && !item.interrupted).length
  const expanded = open ?? (running || live || expandTools)
  return <motion.div className={`tool-group ${expanded ? 'is-open' : ''}`} {...enter}>
    <button className="tool-group-header" aria-expanded={expanded} onClick={() => setOpen(!expanded)}>
      <Layers size={14} /><span className={running ? 'shimmer-text' : ''}>{running ? '正在执行' : '已执行'} {items.length} 个操作</span>
      <small>{summarizeTools(items)}{failed ? ` · ${failed} 个失败` : ''}</small>
    </button>
    <AnimatePresence initial={false}>{expanded && <motion.div className="tool-group-body" {...reveal}>
      {items.map(item => <ToolStep key={item.id} item={item} sessionId={sessionId} clientKey={clientKey} defaultOpen={expandTools} waiting={item.id === waitingId} />)}
    </motion.div>}</AnimatePresence>
  </motion.div>
}
