import { useEffect, useRef, useState } from 'react'

type Breakdown = { system: number; tools: number; messages: number }

const parts: Array<{ key: keyof Breakdown; label: string }> = [
  { key: 'system', label: '系统提示词' },
  { key: 'tools', label: '工具定义' },
  { key: 'messages', label: '对话消息' },
]

function compact(value: number) {
  return new Intl.NumberFormat('en-US', { notation: 'compact', maximumFractionDigits: 1 }).format(value)
}

function approximate(value: number) { return value > 0 ? `~${compact(value)}` : '0' }

export function ContextMeter({ used, windowSize, breakdown, onOpen }: {
  used: number; windowSize: number; breakdown: Breakdown; onOpen: () => void
}) {
  const [open, setOpen] = useState(false)
  const root = useRef<HTMLDivElement>(null)
  const trigger = useRef<HTMLButtonElement>(null)
  const percent = Math.min(100, Math.round(100 * used / Math.max(1, windowSize)))

  useEffect(() => {
    if (!open) return
    const onPointerDown = (event: PointerEvent) => { if (!root.current?.contains(event.target as Node)) setOpen(false) }
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') { event.stopPropagation(); setOpen(false); trigger.current?.focus() }
    }
    document.addEventListener('pointerdown', onPointerDown)
    document.addEventListener('keydown', onKeyDown, true)
    return () => { document.removeEventListener('pointerdown', onPointerDown); document.removeEventListener('keydown', onKeyDown, true) }
  }, [open])

  return <div className="context-meter-wrap" ref={root}>
    <button ref={trigger} type="button" className="context-meter" aria-label="上下文窗口详情" aria-expanded={open} aria-controls="context-breakdown" onClick={() => { if (!open) onOpen(); setOpen(!open) }}>{percent}%</button>
    {open && <div id="context-breakdown" className="context-detail" role="region" aria-label="上下文窗口详情">
      <div className="context-detail-heading"><span>上下文已用 <strong>{percent}%</strong></span><strong>{approximate(used)} / {compact(windowSize)}</strong></div>
      <div className="context-detail-track" role="meter" aria-label="上下文已用" aria-valuenow={percent} aria-valuemin={0} aria-valuemax={100}>
        {parts.map(part => <span key={part.key} className={`context-detail-${part.key}`} style={{ width: `${Math.min(100, 100 * breakdown[part.key] / Math.max(1, windowSize))}%` }} />)}
      </div>
      <div className="context-detail-list">{parts.map(part => <div key={part.key} className="context-detail-row">
        <span className={`context-detail-key context-detail-${part.key}`} /><span>{part.label}</span><strong>{approximate(breakdown[part.key])}</strong>
      </div>)}</div>
      <p className="context-detail-note">按当前提示词估算，实际用量可能不同</p>
    </div>}
  </div>
}
