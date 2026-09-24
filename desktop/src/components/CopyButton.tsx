import { useEffect, useRef, useState } from 'react'
import { Check, Copy } from 'lucide-react'

export function CopyButton({ text, label = '复制' }: { text: string; label?: string }) {
  const [status, setStatus] = useState<'idle' | 'copied' | 'error'>('idle')
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null)
  useEffect(() => () => { if (timer.current) clearTimeout(timer.current) }, [])
  return <button className="copy-button" title={label} aria-label={status === 'copied' ? '已复制' : label} onClick={async () => {
    try { await navigator.clipboard.writeText(text); setStatus('copied') } catch { setStatus('error') }
    if (timer.current) clearTimeout(timer.current)
    timer.current = setTimeout(() => setStatus('idle'), 2000)
  }}>{status === 'copied' ? <Check size={13} /> : <Copy size={13} />}<span aria-live="polite">{status === 'copied' ? '已复制' : status === 'error' ? '复制失败' : label}</span></button>
}
