import type { DesktopEvent, FeedItem } from './types.js'

export type FeedState = {
  items: FeedItem[]
  busy: boolean
  error: string
  startedAt: number | null
  sessionId: string | null
  approvalCallId?: string
}

export const emptyFeed = (): FeedState => ({
  items: [],
  busy: false,
  error: '',
  startedAt: null,
  sessionId: null,
})

function eventTime(value: unknown) {
  const parsed = Date.parse(String(value || ''))
  return Number.isNaN(parsed) ? Date.now() : parsed
}

function upsertAssistant(items: FeedItem[], text: string): FeedItem[] {
  const index = items.findLastIndex(item => item.kind === 'assistant' && item.pending)
  if (index >= 0) {
    return items.map((item, i) => i === index ? { ...item, text: (item.text || '') + text } : item)
  }
  return [...items, { id: `stream-${items.length}`, kind: 'assistant', text, pending: true }]
}

function sealAssistant(items: FeedItem[]): FeedItem[] {
  return items.map(item => item.kind === 'assistant' && item.pending ? { ...item, pending: false } : item)
}

function finishRun(state: FeedState, event: DesktopEvent): FeedState {
  const ended = Date.now()
  const reason = event.result?.exit_reason
  const items = state.items.map(item => ({
    ...item,
    ...(item.approvalId ? { approvalId: undefined, granted: false } : {}),
    ...(item.pending ? { pending: false, interrupted: item.kind === 'tool', endedAt: item.endedAt || ended } : {}),
  }))
  if (event.event === 'run_error') {
    return {
      ...state,
      busy: false,
      startedAt: null,
      error: event.error || '执行失败',
      items: [...items, { id: `error-${ended}`, kind: 'notice', tone: 'error', text: event.error || '执行失败' }],
    }
  }
  if (reason && reason !== 'completed' && reason !== 'cancelled') {
    return {
      ...state,
      busy: false,
      startedAt: null,
      items: [...items, {
        id: `notice-${ended}`,
        kind: 'notice',
        tone: 'warning',
        text: `任务已暂停：${event.result?.error || reason}`,
      }],
    }
  }
  return { ...state, busy: false, startedAt: null, items, approvalCallId: undefined }
}

export function applyDesktopEvent(state: FeedState, event: DesktopEvent): FeedState {
  if (event.event === 'bridge_error') {
    const fatal = event.fatal ?? /exited|不可用|无效消息/.test(event.error || '')
    if (!fatal && !/Traceback|Error|错误/.test(event.error || '')) return state
    if (fatal) return finishRun(state, { event: 'run_error', error: event.error || 'Agent bridge 出错' })
    return { ...state, error: event.error || 'Agent bridge 出错' }
  }
  if (event.sessionId && state.sessionId && event.sessionId !== state.sessionId) return state
  if (event.sessionId && !state.sessionId) state = { ...state, sessionId: event.sessionId }
  if (event.event === 'run_done' || event.event === 'run_error') return finishRun(state, event)
  if (event.event === 'text_delta' && event.text) {
    return { ...state, items: upsertAssistant(state.items, event.text) }
  }
  if (event.event === 'approval' && event.request) {
    return {
      ...state,
      items: [...sealAssistant(state.items), {
        id: `approval-${event.approvalId}`,
        kind: 'approval',
        name: event.request.tool_name,
        args: event.request.arguments,
        approvalId: event.approvalId,
        callId: state.approvalCallId,
      }],
    }
  }
  if (event.event === 'approval_auto' && event.request) {
    const request = event.request
    const index = [...state.items].reverse().findIndex(item => item.kind === 'tool' && item.pending
      && (state.approvalCallId ? (item.callId || item.id) === state.approvalCallId : item.name === request.tool_name))
    if (index < 0) return state
    const actual = state.items.length - 1 - index
    return {
      ...state,
      items: state.items.map((item, i) => i === actual ? { ...item, auto: true } : item),
    }
  }
  if (event.event !== 'agent_event' || !event.item) return state
  return applyAgentEvent(state, event.item.type, event.item.data, event.item.seq, event.item.timestamp)
}

function applyAgentEvent(
  state: FeedState,
  type: string,
  data: Record<string, unknown>,
  seq: number,
  timestamp: string,
): FeedState {
  if (type === 'assistant_message') {
    const index = state.items.findLastIndex(item => item.kind === 'assistant' && item.pending)
    const pending = state.items[index]
    if (!data.text) return { ...state, items: state.items.filter((_, i) => i !== index) }
    const final: FeedItem = { id: pending?.id || `assistant-${seq}`, kind: 'assistant', text: String(data.text) }
    return {
      ...state,
      items: index < 0 ? [...state.items, final] : state.items.map((item, i) => i === index ? final : item),
    }
  }
  if (type === 'tool_call_start') {
    return {
      ...state,
      items: [...sealAssistant(state.items), {
        id: `${data.call_id}:${seq}`,
        callId: String(data.call_id),
        kind: 'tool',
        name: String(data.name),
        args: (data.arguments || {}) as Record<string, unknown>,
        pending: true,
        startedAt: eventTime(timestamp),
      }],
    }
  }
  if (type === 'tool_call_result') {
    const index = state.items.findLastIndex(item => item.kind === 'tool' && (item.callId || item.id) === String(data.call_id))
    const output = data.output_detail || data.output_preview
    const error = data.error !== output ? data.error : undefined
    return {
      ...state,
      items: state.items.map((item, i) => i === index ? {
        ...item,
        pending: false,
        success: Boolean(data.success),
        endedAt: eventTime(timestamp),
        output: [output, error].filter(Boolean).join('\n'),
        artifactId: data.artifact_id ? String(data.artifact_id) : undefined,
      } : item),
    }
  }
  if (type === 'tool_output') {
    const index = state.items.findLastIndex(item => item.kind === 'tool' && item.pending && (item.callId || item.id) === String(data.call_id))
    return {
      ...state,
      items: state.items.map((item, i) => i === index
        ? { ...item, output: String(data.output_preview || '') }
        : item),
    }
  }
  if (type === 'approval_request') return { ...state, approvalCallId: String(data.call_id) }
  if (type === 'approval_decision') {
    const index = state.items.findIndex(item => item.kind === 'approval' && item.approvalId
      && (!item.callId || item.callId === String(data.call_id)))
    return {
      ...state,
      items: state.items.map((item, i) => i === index
        ? { ...item, approvalId: undefined, granted: Boolean(data.granted) }
        : item),
    }
  }
  if (type === 'subagent_start') {
    const index = state.items.findLastIndex(item => item.kind === 'tool' && item.pending && item.name === 'delegate'
      && item.args?.kind === data.kind && item.args?.task === data.task)
    return {
      ...state,
      items: state.items.map((item, i) => i === index
        ? { ...item, childSessionId: String(data.child_session_id) }
        : item),
    }
  }
  if (type === 'provider_retry') {
    return {
      ...state,
      items: [...state.items.filter(item => !(item.kind === 'assistant' && item.pending)), {
        id: `retry-${seq}`,
        kind: 'notice',
        tone: 'warning',
        text: `模型请求暂时失败，${Number(data.delay_s || 0)} 秒后重试（第 ${Number(data.next_attempt || 2)} 次）。`,
      }],
    }
  }
  const notice = type === 'mcp_discovery' && data.error ? String(data.error)
    : type === 'context_compacted' ? '上下文已自动压缩，较早的内容已归档'
    : type === 'goal_check' ? `验收检查：${data.passed ? '通过' : '仍需改进'}`
    : type === 'background_job_completed' || type === 'background_job_lost'
      ? `后台命令 ${data.job_id}：${type === 'background_job_completed' ? '已完成' : '结果丢失'}`
    : type === 'side_effect_unknown' ? `工具执行结果需复核：${data.name || data.call_id || ''}`
    : null
  if (notice) {
    return {
      ...state,
      items: [...state.items, {
        id: `notice-${seq}`,
        kind: 'notice',
        text: notice,
        tone: type === 'mcp_discovery' || type === 'side_effect_unknown' || type === 'background_job_lost' ? 'warning' : 'info',
      }],
    }
  }
  return state
}

export function pendingApproval(items: FeedItem[]): FeedItem | undefined {
  return items.find(item => item.kind === 'approval' && item.approvalId)
}

export function isLive(item: FeedItem): boolean {
  if (item.kind === 'assistant' && item.pending) return true
  if (item.kind === 'tool' && item.pending) return true
  if (item.kind === 'approval' && item.approvalId) return true
  return false
}
