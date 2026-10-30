import type { FeedItem } from './types.js'

export type ChildStep = { id: string; name: string; status: string; firstSeq: number; seq: number }

export function applyChildEvent(items: FeedItem[], type: string, data: Record<string, unknown>): FeedItem[] {
  if (!['subagent_start', 'subagent_progress', 'subagent_result'].includes(type)) return items
  const call = String(data.call_id || '')
  const child = String(data.child_session_id || '')
  const index = items.findLastIndex(item => item.kind === 'tool' && item.name === 'delegate'
    && (call ? (item.callId || item.id) === call : child && item.childSessionId === child
      || type === 'subagent_start' && item.pending && item.args?.kind === data.kind && item.args?.task === data.task))
  if (index < 0) return items
  let item = { ...items[index] }
  if (child && item.childSessionId && item.childSessionId !== child) {
    item = { ...item, childProgress: undefined, childSeq: undefined, childStatus: undefined, childFinished: undefined }
  }
  if (child) item.childSessionId = child
  if (type === 'subagent_result') {
    item.childStatus = String(data.exit_reason || 'completed')
    item.childFinished = true
    item.childProgress = item.childProgress?.map(step => step.status === 'running' ? { ...step, status: 'interrupted' } : step)
  } else if (type === 'subagent_progress') {
    const seq = Number(data.child_event_seq)
    if (!Number.isSafeInteger(seq) || seq < 0) return items
    if (!item.childFinished && seq > (item.childSeq ?? -1)) {
      item.childSeq = seq
      item.childStatus = data.phase === 'model' ? 'model' : String(data.name || 'tool')
    }
    if (data.phase === 'tool' && data.child_call_id) {
      const id = String(data.child_call_id)
      const steps = [...(item.childProgress || [])]
      const at = steps.findIndex(step => step.id === id)
      const old = steps[at]
      const status = item.childFinished && data.status === 'running' ? 'interrupted' : String(data.status)
      const next = { id, name: String(data.name || old?.name || 'tool'), status, firstSeq: Math.min(seq, old?.firstSeq ?? seq), seq }
      if (old) steps[at] = seq > old.seq ? next : { ...old, firstSeq: next.firstSeq }
      else steps.push(next)
      item.childProgress = steps.sort((a, b) => a.firstSeq - b.firstSeq)
    }
  }
  const result = items.slice()
  result[index] = item
  return result
}

export function childStatusLabel(status = ''): string {
  return ({ model: '思考中', completed: '已完成', failed: '失败', interrupted: '已中断', cancelled: '已取消', error: '失败', internal_error: '失败', provider_error: '模型请求失败', running: '运行中', max_rounds: '已暂停', time_budget: '已暂停', token_budget: '已暂停', context_limit: '已暂停' } as Record<string, string>)[status] || status || '准备中'
}

export function childSummary(item: FeedItem): string {
  return `子任务：${childStatusLabel(item.childStatus)} · ${item.childProgress?.length || 0} 次工具调用`
}
