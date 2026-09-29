import { useEffect, useRef, useState } from 'react'
import { AnimatePresence, motion } from 'motion/react'
import { compactNumber } from '../../lib/format'

type Breakdown = { system: number; tools: number; messages: number }
const parts: Array<{ key: keyof Breakdown; label: string }> = [
  { key: 'system', label: '系统提示词' }, { key: 'tools', label: '工具定义' }, { key: 'messages', label: '对话消息' },
]
const approximate = (value: number) => value > 0 ? `~${compactNumber(value)}` : '0'

export function ContextMeter({ used, windowSize, breakdown, usage, onOpen, onCompact }: {
  used: number; windowSize: number; breakdown: Breakdown; usage: { input_tokens: number; output_tokens: number } | null
  onOpen: () => void; onCompact?: () => void
}) {
  const [open, setOpen] = useState(false)
  const root = useRef<HTMLDivElement>(null)
  const trigger = useRef<HTMLButtonElement>(null)
  const ratio = Math.min(1, used / Math.max(1, windowSize))
  const percent = Math.round(ratio * 100)
  const level = percent >= 85 ? 'danger' : percent >= 65 ? 'warning' : 'normal'
  useEffect(() => {
    if (!open) return
    const onPointerDown = (event: PointerEvent) => { if (!root.current?.contains(event.target as Node)) setOpen(false) }
    const onKeyDown = (event: KeyboardEvent) => { if (event.key === 'Escape') { event.stopPropagation(); setOpen(false); trigger.current?.focus() } }
    document.addEventListener('pointerdown', onPointerDown)
    document.addEventListener('keydown', onKeyDown, true)
    return () => { document.removeEventListener('pointerdown', onPointerDown); document.removeEventListener('keydown', onKeyDown, true) }
  }, [open])
  const circumference = 2 * Math.PI * 7
  return <div className="context-meter-wrap" ref={root}>
    <button ref={trigger} type="button" className={`context-meter context-${level}`} aria-label="上下文窗口详情" aria-expanded={open} aria-controls="context-breakdown"
      data-tip={open ? undefined : `上下文已用 ${percent}%`} onClick={() => { if (!open) onOpen(); setOpen(!open) }}>
      <svg width="18" height="18" viewBox="0 0 18 18" aria-hidden="true">
        <circle cx="9" cy="9" r="7" className="ring-track" />
        <circle cx="9" cy="9" r="7" className="ring-value" strokeDasharray={circumference} strokeDashoffset={circumference * (1 - ratio)} />
      </svg>
      <span>{percent}%</span>
    </button>
    <AnimatePresence>{open && <motion.div id="context-breakdown" className="context-detail" role="region" aria-label="上下文窗口详情"
      initial={{ opacity: 0, y: 6, scale: .98 }} animate={{ opacity: 1, y: 0, scale: 1 }} exit={{ opacity: 0, y: 4, scale: .98 }} transition={{ duration: .14 }}>
      <div className="context-detail-heading"><span>上下文窗口</span><strong>{approximate(used)} / {compactNumber(windowSize)}</strong></div>
      <div className="context-detail-track" role="meter" aria-label="上下文已用" aria-valuenow={percent} aria-valuemin={0} aria-valuemax={100}>
        {parts.map(part => <motion.span key={part.key} className={`context-part-${part.key}`} initial={{ width: 0 }} animate={{ width: `${Math.min(100, 100 * breakdown[part.key] / Math.max(1, windowSize))}%` }} transition={{ duration: .4, ease: [.2, .8, .2, 1] }} />)}
      </div>
      <div className="context-detail-list">{parts.map(part => <div key={part.key} className="context-detail-row">
        <span className={`context-key context-part-${part.key}`} /><span>{part.label}</span><strong>{approximate(breakdown[part.key])}</strong>
      </div>)}</div>
      {usage && <div className="context-usage"><span>本会话累计</span><strong>↑ {compactNumber(usage.input_tokens)}　↓ {compactNumber(usage.output_tokens)}</strong></div>}
      <p className="context-detail-note">按当前提示词估算，实际用量可能不同。{percent >= 65 ? '接近上限时会自动压缩较早的内容。' : ''}</p>
      {onCompact && percent >= 40 && <button className="button button-ghost button-small context-compact" onClick={() => { setOpen(false); onCompact() }}>立即压缩上下文</button>}
    </motion.div>}</AnimatePresence>
  </div>
}
