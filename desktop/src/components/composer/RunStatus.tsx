import { motion } from 'motion/react'
import { ShieldAlert, Square } from 'lucide-react'
import { useNow } from '../../hooks/useNow'
import { clock, modKey } from '../../lib/format'

export function RunStatus({ approval, onStop, startedAt }: { approval: boolean; onStop: () => void; startedAt?: number | null }) {
  const now = useNow(true)
  const seconds = startedAt ? Math.max(0, Math.floor((now - startedAt) / 1000)) : 0
  return <motion.div className={`run-status ${approval ? 'is-waiting' : ''}`} initial={{ opacity: 0, y: 8, scale: .97 }} animate={{ opacity: 1, y: 0, scale: 1 }} exit={{ opacity: 0, y: 6, scale: .97 }}
    transition={{ type: 'spring', stiffness: 500, damping: 36 }}>
    {approval ? <ShieldAlert size={13} /> : <span className="run-spinner" aria-hidden="true" />}
    <span role="status" className={approval ? '' : 'shimmer-text'}>{approval ? '等待你的批准' : '正在处理任务'}</span>
    <time>{clock(seconds)}</time>
    <button onClick={onStop} aria-label="停止任务" data-tip="停止任务" data-shortcut={`${modKey} .`}><Square size={9} fill="currentColor" />停止</button>
  </motion.div>
}
