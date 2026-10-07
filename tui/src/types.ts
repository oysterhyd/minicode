export type Usage = {
  input_tokens: number
  output_tokens: number
  cache_read_tokens?: number
  cache_write_tokens?: number
  available?: boolean
}

export type AgentState = {
  protocolVersion?: number
  runId?: string | null
  running?: boolean
  phase?: 'idle' | 'preparing' | 'model' | 'tools' | 'approval' | 'finishing' | 'paused'
  model: string
  effort: string
  permissionMode: string
  sessionId: string | null
  taskPending: boolean
  rounds: number
  contextTokens: number
  contextWindow: number
  contextBreakdown: { system: number; tools: number; messages: number }
  usage: Usage | null
  budget: { max_rounds: number; max_total_tokens: number; max_seconds: number }
  acceptance: string
  alwaysAllow?: string[]
}

export type Command = { name: string; usage: string; summary: string }
export type SessionSummary = { session_id: string; title?: string }
export type ModelInfo = { id: string; available?: boolean; provider?: string }
export type SkillInfo = { name: string; description: string }

export type AgentEvent = { seq: number; type: string; timestamp: string; data: Record<string, unknown> }
export type Message = { role: 'user' | 'assistant'; content: Array<Record<string, unknown>> }

export type FeedItem = {
  id: string
  callId?: string
  kind: 'user' | 'assistant' | 'tool' | 'approval' | 'notice'
  text?: string
  name?: string
  args?: Record<string, unknown>
  output?: string
  success?: boolean
  pending?: boolean
  interrupted?: boolean
  approvalId?: string
  granted?: boolean
  auto?: boolean
  childSessionId?: string
  artifactId?: string
  startedAt?: number
  endedAt?: number
  tone?: 'info' | 'warning' | 'error'
}

export type ApprovalRequestData = { tool_name: string; arguments: Record<string, unknown>; summary: string }

export type DesktopEvent = {
  clientKey?: string
  event: 'agent_event' | 'text_delta' | 'approval' | 'approval_auto' | 'run_done' | 'run_error' | 'bridge_error'
  sessionId?: string
  item?: AgentEvent
  text?: string
  approvalId?: string
  request?: ApprovalRequestData
  result?: { exit_reason: string; error?: string }
  error?: string
  fatal?: boolean
}

export type SessionDetail = {
  summary: { session_id: string; workspace: string; model: string }
  messages: Message[]
  events: AgentEvent[]
}

export type SlashResult = { action?: string; message?: string; sessionId?: string; state?: AgentState }

export type InitializeResult = {
  sessions: SessionSummary[]
  models: ModelInfo[]
  defaultModel: string
  commands: Command[]
  state: AgentState
}

export type Capabilities = {
  skills: SkillInfo[]
}

function time(value: unknown) {
  const parsed = Date.parse(String(value || ''))
  return Number.isNaN(parsed) ? undefined : parsed
}

export function historyItems(detail: SessionDetail): FeedItem[] {
  const results = new Map<string, AgentEvent>()
  const starts = new Map<string, AgentEvent>()
  const decisions = new Map<string, AgentEvent>()
  const children: AgentEvent[] = []
  const messageResults = new Map<string, Record<string, unknown>>()
  for (const message of detail.messages) {
    for (const block of message.content) {
      if (block.type === 'tool_result') messageResults.set(String(block.tool_use_id), block)
    }
  }
  for (const event of detail.events) {
    if (event.type === 'tool_call_start') starts.set(String(event.data.call_id), event)
    if (event.type === 'tool_call_result') results.set(String(event.data.call_id), event)
    if (event.type === 'approval_decision') decisions.set(String(event.data.call_id), event)
    if (event.type === 'subagent_start') children.push(event)
  }
  const items: FeedItem[] = []
  detail.messages.forEach((message, index) => {
    message.content.forEach((block, blockIndex) => {
      const id = `${index}-${blockIndex}`
      if (block.type === 'text' && block.text) {
        items.push({ id, kind: message.role === 'assistant' ? 'assistant' : 'user', text: String(block.text) })
      }
      if (block.type === 'tool_use') {
        const callId = String(block.id)
        const input = (block.input || {}) as Record<string, unknown>
        const decision = decisions.get(callId)
        if (decision && !decision.data.granted) {
          items.push({ id: `${id}-approval`, kind: 'approval', name: String(block.name), args: input, granted: false })
        }
        const result = results.get(callId)
        const persisted = messageResults.get(callId)
        const start = starts.get(callId)
        const child = children.find(event => event.data.kind === input.kind && event.data.task === input.task
          && event.seq > (start?.seq ?? -1) && event.seq < (result?.seq ?? Infinity))
        const output = result?.data.output_detail || result?.data.output_preview || persisted?.content || ''
        const error = result?.data.error
        items.push({
          id: callId,
          callId,
          kind: 'tool',
          name: String(block.name),
          args: input,
          childSessionId: child ? String(child.data.child_session_id) : undefined,
          artifactId: result?.data.artifact_id ? String(result.data.artifact_id) : undefined,
          output: [output, error && error !== output ? error : ''].filter(Boolean).join('\n') || undefined,
          success: result ? Boolean(result.data.success) : persisted ? !persisted.is_error : undefined,
          interrupted: !result && !persisted,
          startedAt: time(start?.timestamp),
          endedAt: time(result?.timestamp),
          pending: false,
        })
      }
    })
  })
  return items
}

export const emptyState: AgentState = {
  model: '',
  effort: 'off',
  permissionMode: 'default',
  sessionId: null,
  taskPending: false,
  rounds: 0,
  contextTokens: 0,
  contextWindow: 200000,
  contextBreakdown: { system: 0, tools: 0, messages: 0 },
  usage: null,
  budget: { max_rounds: 0, max_total_tokens: 0, max_seconds: 0 },
  acceptance: '',
  alwaysAllow: [],
}
