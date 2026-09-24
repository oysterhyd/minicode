import { useEffect, useState } from 'react'
import { LoaderCircle, ShieldCheck, Square } from 'lucide-react'

export function RunStatus({ approval, onStop, startedAt }: { approval: boolean; onStop: () => void; startedAt?: number | null }) {
  const [fallback] = useState(Date.now)
  const started = startedAt || fallback
  const [seconds, setSeconds] = useState(0)
  useEffect(() => {
    const timer = window.setInterval(() => setSeconds(Math.floor((Date.now() - started) / 1000)), 1000)
    return () => clearInterval(timer)
  }, [started])
  return <div className={`run-status ${approval ? 'awaiting-approval' : ''}`}>
    {approval ? <ShieldCheck size={14} /> : <LoaderCircle size={14} className="animate-spin" />}
    <span role="status">{approval ? '等待你的批准' : '正在处理任务'}</span><time>{Math.floor(seconds / 60)}:{String(seconds % 60).padStart(2, '0')}</time>
    <button onClick={onStop} aria-label="停止任务"><Square size={10} fill="currentColor" />停止</button>
  </div>
}
