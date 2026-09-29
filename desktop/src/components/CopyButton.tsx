import { useEffect, useRef, useState } from 'react'
import { AnimatePresence, motion } from 'motion/react'
import { Check, Copy } from 'lucide-react'

export function CopyButton({ text, label = '复制', iconOnly = false }: { text: string; label?: string; iconOnly?: boolean }) {
  const [status, setStatus] = useState<'idle' | 'copied' | 'error'>('idle')
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null)
  useEffect(() => () => { if (timer.current) clearTimeout(timer.current) }, [])
  const text_ = status === 'copied' ? '已复制' : status === 'error' ? '复制失败' : label
  return <button className={`copy-button ${iconOnly ? 'copy-button-icon' : ''} ${status === 'copied' ? 'is-copied' : ''}`} aria-label={status === 'copied' ? '已复制' : label}
    data-tip={iconOnly ? text_ : undefined} onClick={async event => {
      event.stopPropagation()
      try { await navigator.clipboard.writeText(text); setStatus('copied') } catch { setStatus('error') }
      if (timer.current) clearTimeout(timer.current)
      timer.current = setTimeout(() => setStatus('idle'), 1800)
    }}>
    <AnimatePresence initial={false} mode="popLayout">
      <motion.span key={status === 'copied' ? 'check' : 'copy'} className="copy-icon" initial={{ scale: .6, opacity: 0 }} animate={{ scale: 1, opacity: 1 }} exit={{ scale: .6, opacity: 0 }} transition={{ duration: .14 }}>
        {status === 'copied' ? <Check size={13} /> : <Copy size={13} />}
      </motion.span>
    </AnimatePresence>
    {!iconOnly && <span aria-live="polite">{text_}</span>}
  </button>
}
