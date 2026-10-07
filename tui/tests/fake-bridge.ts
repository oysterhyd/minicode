import { EventEmitter } from 'node:events'
import type { BridgeApi } from '../src/bridge.js'
import type { DesktopEvent } from '../src/types.js'

export class FakeBridge implements BridgeApi {
  handlers = new Map<string, (params: Record<string, unknown>) => unknown | Promise<unknown>>()
  requests: Array<{ method: string; params: Record<string, unknown> }> = []
  private events = new EventEmitter()
  closed = false

  constructor() {
    this.handlers.set('initialize', () => ({
      sessions: [],
      models: [{ id: 'fake', available: true }],
      defaultModel: 'fake',
      commands: [
        { name: '/help', usage: '/help', summary: '显示本帮助' },
        { name: '/model', usage: '/model [名称]', summary: '查看或切换活跃模型' },
        { name: '/effort', usage: '/effort [off|low|medium|high|xhigh|max]', summary: '调整推理预算' },
        { name: '/permissions', usage: '/permissions [default|accept_edits|bypass]', summary: '查看或切换权限模式' },
        { name: '/skill', usage: '/skill [名称]', summary: '列出或激活技能' },
        { name: '/resume', usage: '/resume <会话ID8>', summary: '恢复一个历史会话' },
        { name: '/exit', usage: '/exit', summary: '退出' },
      ],
      state: {
        model: 'fake',
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
      },
    }))
    this.handlers.set('setModel', params => ({
      model: params.model,
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
    }))
    this.handlers.set('setTuiProvider', params => this.handlers.get('setModel')!({ model: params.model || 'fake' }))
    this.handlers.set('listSessions', () => [])
    this.handlers.set('setPermissionMode', params => ({
      model: 'fake',
      effort: 'off',
      permissionMode: params.mode,
      sessionId: null,
      taskPending: false,
      rounds: 0,
      contextTokens: 0,
      contextWindow: 200000,
      contextBreakdown: { system: 0, tools: 0, messages: 0 },
      usage: null,
      budget: { max_rounds: 0, max_total_tokens: 0, max_seconds: 0 },
      acceptance: '',
    }))
    this.handlers.set('getState', () => this.handlers.get('initialize')!({}).state)
    this.handlers.set('getCapabilities', () => ({
      skills: [{ name: 'review', description: '代码审查' }],
    }))
    this.handlers.set('sendPrompt', () => ({ sessionId: 'sess-1' }))
    this.handlers.set('resolveApproval', () => true)
    this.handlers.set('cancelTurn', () => true)
    this.handlers.set('runSlash', params => {
      const text = String(params.text || '')
      if (text.startsWith('/exit')) return { action: 'exit' }
      if (text.startsWith('/help')) return { message: '可用命令：\n  /help' }
      return { message: 'ok' }
    })
  }

  async request<T = unknown>(method: string, params: Record<string, unknown> = {}): Promise<T> {
    this.requests.push({ method, params })
    const handler = this.handlers.get(method)
    if (!handler) throw new Error(`未知请求：${method}`)
    return handler(params) as T
  }

  onEvent(listener: (event: DesktopEvent) => void): () => void {
    this.events.on('event', listener)
    return () => this.events.off('event', listener)
  }

  emit(event: DesktopEvent): void {
    this.events.emit('event', event)
  }

  close(): void {
    this.closed = true
  }
}

export function delay(ms = 20): Promise<void> {
  return new Promise(resolve => setTimeout(resolve, ms))
}
