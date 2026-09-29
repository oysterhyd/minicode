import { useEffect, useMemo, useRef, useState } from 'react'
import { motion } from 'motion/react'
import { Check, CheckCheck, LoaderCircle, ShieldAlert, ShieldCheck, ShieldX, X } from 'lucide-react'
import type { FeedItem } from '../../types'
import { diffTexts } from '../../lib/diff'
import { highlight } from '../../lib/highlight'
import { languageFor, toolMeta, toolTarget } from '../../lib/tools'
import { DiffView } from '../DiffView'

export function ApprovalCard({ item, onDecide }: { item: FeedItem; onDecide: (id: string, granted: boolean, remember?: boolean) => Promise<void> | void }) {
  const [submitting, setSubmitting] = useState<'approve' | 'always' | 'reject' | null>(null)
  const approve = useRef<HTMLButtonElement>(null)
  const lines = useMemo(() => item.name === 'edit' ? diffTexts(String(item.args?.old_text ?? ''), String(item.args?.new_text ?? '')) : null, [item.name, item.args])
  const content = item.name === 'write' ? String(item.args?.content ?? '') : ''
  const preview = useMemo(() => content ? highlight(content.split('\n').slice(0, 40).join('\n'), languageFor(String(item.args?.path || ''))) : null, [content])
  const pending = Boolean(item.approvalId)
  useEffect(() => {
    // Bring the decision into reach unless the user is typing.
    if (pending && (document.activeElement === document.body || !document.activeElement)) approve.current?.focus({ preventScroll: true })
  }, [pending])
  async function decide(granted: boolean, remember = false) {
    if (!item.approvalId || submitting) return
    setSubmitting(granted ? remember ? 'always' : 'approve' : 'reject')
    try { await onDecide(item.approvalId, granted, remember) } finally { setSubmitting(null) }
  }
  if (!pending) {
    const Icon = item.granted ? ShieldCheck : ShieldX
    return <div className={`approval-resolved ${item.granted ? 'is-granted' : 'is-denied'}`}><Icon size={14} />
      <span>{item.granted ? '已批准' : '已拒绝'} · {toolMeta(item.name).done} <code>{toolTarget(item)}</code></span></div>
  }
  const title = item.name === 'bash' ? '运行终端命令' : item.name === 'edit' ? '编辑文件' : item.name === 'write' ? '写入文件' : `调用 ${item.name}`
  return <motion.div className="approval-card" role="group" aria-label="需要你的批准" initial={{ opacity: 0, y: 8, scale: .98 }} animate={{ opacity: 1, y: 0, scale: 1 }}
    transition={{ type: 'spring', stiffness: 460, damping: 34 }}
    onKeyDown={event => {
      if (event.target instanceof HTMLTextAreaElement) return
      if (event.key.toLowerCase() === 'y') { event.preventDefault(); void decide(true) }
      if (event.key.toLowerCase() === 'a') { event.preventDefault(); void decide(true, true) }
      if (event.key.toLowerCase() === 'n' || event.key === 'Escape') { event.preventDefault(); void decide(false) }
    }}>
    <header className="approval-header">
      <span className="approval-icon"><ShieldAlert size={16} /></span>
      <div><strong>需要你的批准</strong><span>Agent 请求{title}</span></div>
      <span className="approval-badge"><span className="pulse-dot" />等待确认</span>
    </header>
    <div className="approval-body">
      {item.name === 'bash' ? <div className="terminal-block"><div className="terminal-line"><span className="terminal-prompt">$</span><code>{String(item.args?.command || '')}</code></div>
        {item.args?.cwd ? <div className="approval-meta">目录 {String(item.args.cwd)}</div> : null}</div>
        : <>
          {item.args?.path ? <div className="approval-path">{String(item.args.path)}</div> : null}
          {lines ? <DiffView lines={lines} maxLines={40} compact /> : content ? <pre className="code-preview hljs">{preview}</pre>
            : <pre className="tool-args">{JSON.stringify(item.args || {}, null, 2)}</pre>}
        </>}
    </div>
    <footer className="approval-actions">
      <button ref={approve} className="button button-primary" disabled={Boolean(submitting)} onClick={() => void decide(true)}>
        {submitting === 'approve' ? <LoaderCircle size={14} className="spin" /> : <Check size={14} />}批准执行<kbd>Y</kbd></button>
      <button className="button button-secondary" disabled={Boolean(submitting)} onClick={() => void decide(true, true)}>
        {submitting === 'always' ? <LoaderCircle size={14} className="spin" /> : <CheckCheck size={14} />}本会话始终允许 {item.name}<kbd>A</kbd></button>
      <button className="button button-ghost" disabled={Boolean(submitting)} onClick={() => void decide(false)}>
        {submitting === 'reject' ? <LoaderCircle size={14} className="spin" /> : <X size={14} />}拒绝<kbd>N</kbd></button>
    </footer>
  </motion.div>
}
