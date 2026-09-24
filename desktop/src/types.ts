export type Session = {
  session_id: string
  title: string
  workspace: string
  provider: string
  model: string
  status: string
  created_at: string
}

export type Model = { id: string; provider: string; available: boolean }
export type AgentState = { model: string; effort: string; permissionMode: string; sessionId: string | null; taskPending: boolean; rounds: number; contextTokens: number; contextWindow: number; usage: { input_tokens: number; output_tokens: number } | null; budget: { max_rounds: number; max_total_tokens: number; max_seconds: number }; acceptance: string }
export type Command = { name: string; usage: string; summary: string }
export type Capabilities = {
  skills: Array<{ name: string; description: string; origin: string; active: boolean }>
  plugins: Array<{ name: string; version: string; enabled: boolean; digest: string; path: string; skills: boolean; agents: boolean; servers: string[] }>
  mcp: Array<{ name: string; plugin: string }>
  agents: string[]
}
export type SlashResult = { action?: string; message?: string; sessionId?: string; state?: AgentState }
export type Change = { status: string; path: string }
export type SessionTask = { task_id: string; title: string; status: 'pending' | 'ready' | 'running' | 'done' | 'failed' }
export type AgentEvent = { seq: number; type: string; timestamp: string; data: Record<string, unknown> }
export type Message = { role: 'user' | 'assistant'; content: Array<Record<string, unknown>> }
export type FeedItem = {
  id: string
  kind: 'user' | 'assistant' | 'tool' | 'approval' | 'notice' | 'thinking'
  text?: string
  name?: string
  args?: Record<string, unknown>
  output?: string
  success?: boolean
  pending?: boolean
  approvalId?: string
  granted?: boolean
  childSessionId?: string
  artifactId?: string
}

export type DesktopEvent = {
  event: 'agent_event' | 'text_delta' | 'approval' | 'run_done' | 'run_error' | 'bridge_error'
  sessionId?: string
  item?: AgentEvent
  text?: string
  approvalId?: string
  request?: { tool_name: string; arguments: Record<string, unknown>; summary: string }
  result?: { exit_reason: string; error?: string }
  error?: string
}

export type SessionDetail = { summary: Session; messages: Message[]; events: AgentEvent[] }

declare global {
  interface Window {
    desktop: {
      request: <T = unknown>(method: string, params?: Record<string, unknown>) => Promise<T>
      chooseWorkspace: () => Promise<string | null>
      chooseAcceptanceFile: () => Promise<string | null>
      onEvent: (listener: (event: DesktopEvent) => void) => () => void
    }
  }
}

export function historyItems(detail: SessionDetail): FeedItem[] {
  const results = new Map<string, AgentEvent>()
  const decisions = new Map<string, AgentEvent>()
  const children = new Map<string, string>()
  for (const event of detail.events) {
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
        const decision = decisions.get(callId)
        if (decision) {
          items.push({ id: `${id}-approval`, kind: 'approval', name: String(block.name),
            args: block.input as Record<string, unknown>, granted: Boolean(decision.data.granted) })
        }
        const result = results.get(callId)
        items.push({ id: callId, kind: 'tool', name: String(block.name), args: block.input as Record<string, unknown>,
          childSessionId: children.get(`${(block.input as Record<string, unknown>).kind}:${(block.input as Record<string, unknown>).task}`),
          artifactId: result?.data.artifact_id ? String(result.data.artifact_id) : undefined,
          output: result ? String(result.data.output_detail || result.data.error || '') : undefined,
          success: result ? Boolean(result.data.success) : undefined,
          pending: !result })
      }
    })
  })
  return items
}
