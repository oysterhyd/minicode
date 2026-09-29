export type Session = {
  session_id: string
  title: string
  workspace: string
  provider: string
  model: string
  status: string
  created_at: string
  updated_at?: string
  pinned?: boolean
  custom_title?: boolean
}

export type Model = { id: string; provider: string; available: boolean; supportsEffort: boolean }
export type AgentState = {
  model: string; effort: string; permissionMode: string; sessionId: string | null; taskPending: boolean; rounds: number
  contextTokens: number; contextWindow: number; contextBreakdown: { system: number; tools: number; messages: number }
  usage: { input_tokens: number; output_tokens: number } | null
  budget: { max_rounds: number; max_total_tokens: number; max_seconds: number }; acceptance: string; alwaysAllow?: string[]
}
export type Command = { name: string; usage: string; summary: string }
export type Capabilities = {
  skills: Array<{ name: string; description: string; origin: string; active: boolean }>
  plugins: Array<{ name: string; version: string; enabled: boolean; digest: string; path: string; skills: boolean; agents: boolean; servers: string[] }>
  mcp: Array<{ name: string; plugin: string }>
  agents: string[]
}
export type McpDiscovery = { servers: Array<{ server: string; protocol: string | null; server_version: string | null; error: string | null }>; tools: string[] }
export type SlashResult = { action?: string; message?: string; sessionId?: string; state?: AgentState }
export type Change = { status: string; path: string; additions?: number; deletions?: number }
export type SessionTask = { task_id: string; title: string; status: 'pending' | 'ready' | 'running' | 'done' | 'failed' }
export type AgentEvent = { seq: number; type: string; timestamp: string; data: Record<string, unknown> }
export type Message = { role: 'user' | 'assistant'; content: Array<Record<string, unknown>> }
export type FeedItem = {
  id: string
  kind: 'user' | 'assistant' | 'tool' | 'approval' | 'notice' | 'thinking' | 'summary'
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
  event: 'agent_event' | 'text_delta' | 'approval' | 'approval_auto' | 'run_done' | 'run_error' | 'bridge_error' | 'notification_click'
  sessionId?: string
  item?: AgentEvent
  text?: string
  approvalId?: string
  request?: ApprovalRequestData
  result?: { exit_reason: string; error?: string }
  error?: string
}

export type SessionDetail = { summary: Session; messages: Message[]; events: AgentEvent[] }
export type AppInfo = { version: string; electron: string; chrome: string; platform: string }

declare global {
  interface Window {
    desktop: {
      request: <T = unknown>(method: string, params?: Record<string, unknown>) => Promise<T>
      chooseWorkspace: () => Promise<string | null>
      chooseAcceptanceFile: () => Promise<string | null>
      getPathForFile?: (file: File) => string
      onEvent: (listener: (event: DesktopEvent) => void) => () => void
    }
  }
}

function time(value: unknown) {
  const parsed = Date.parse(String(value || ''))
  return Number.isNaN(parsed) ? undefined : parsed
}

export function historyItems(detail: SessionDetail): FeedItem[] {
  const results = new Map<string, AgentEvent>()
  const starts = new Map<string, AgentEvent>()
  const decisions = new Map<string, AgentEvent>()
  const children = new Map<string, string>()
  for (const event of detail.events) {
    if (event.type === 'tool_call_start') starts.set(String(event.data.call_id), event)
    if (event.type === 'tool_call_result') results.set(String(event.data.call_id), event)
    if (event.type === 'approval_decision') decisions.set(String(event.data.call_id), event)
    if (event.type === 'subagent_start') children.set(`${event.data.kind}:${event.data.task}`, String(event.data.child_session_id))
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
        items.push({ id: callId, kind: 'tool', name: String(block.name), args: input,
          childSessionId: children.get(`${input.kind}:${input.task}`),
          artifactId: result?.data.artifact_id ? String(result.data.artifact_id) : undefined,
          output: result ? String(result.data.output_detail || result.data.error || '') : undefined,
          success: result ? Boolean(result.data.success) : undefined,
          startedAt: time(starts.get(callId)?.timestamp), endedAt: time(result?.timestamp),
          pending: !result })
      }
    })
  })
  return items
}
