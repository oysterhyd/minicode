import { useState } from 'react'
import { Check, LoaderCircle, ShieldAlert, X } from 'lucide-react'
import type { FeedItem } from '../types'

export function ToolApprovalCard({ item, onDecide }: { item: FeedItem; onDecide: (id: string, granted: boolean) => void }) {
  const [submitting, setSubmitting] = useState(false)
  async function decide(granted: boolean) {
    if (!item.approvalId || submitting) return
    setSubmitting(true)
    try { await onDecide(item.approvalId, granted) } finally { setSubmitting(false) }
  }
  const detail = item.name === 'bash' ? String(item.args?.command || '') :
    item.name === 'edit' ? `${String(item.args?.path || '')}\n- ${String(item.args?.old_text || '')}\n+ ${String(item.args?.new_text || '')}` :
    item.name === 'write' ? `${String(item.args?.path || '')}\n${String(item.args?.content || '')}` :
    String(item.args?.path || item.args?.file_path || JSON.stringify(item.args || {}))
  return <div className="approval-card">
    <div className="flex items-start gap-3">
      <div className="approval-icon"><ShieldAlert size={18} /></div>
      <div className="min-w-0 flex-1">
        <div className="text-sm font-semibold text-white">{item.approvalId ? '需要你的批准' : item.granted ? '操作已批准' : '操作已拒绝'}</div>
        <div className="mt-0.5 text-xs text-secondary">{item.name === 'bash' ? 'Agent 请求运行终端命令' : `Agent 请求执行 ${item.name} 操作`}</div>
      </div>
      {item.approvalId && <span className="approval-badge">等待确认</span>}
    </div>
    <pre className="code-box mt-4 max-h-44 overflow-auto">{detail}</pre>
    {item.approvalId ? <div className="mt-4 flex gap-2">
      <button className="approve-button" disabled={submitting} onClick={() => void decide(true)}>{submitting ? <LoaderCircle size={15} className="animate-spin" /> : <Check size={15} />}批准执行</button>
      <button className="reject-button" disabled={submitting} onClick={() => void decide(false)}><X size={15} />拒绝</button>
    </div> : <div className="mt-3 text-xs text-secondary">{item.granted ? '已批准' : '已拒绝'}</div>}
  </div>
}
