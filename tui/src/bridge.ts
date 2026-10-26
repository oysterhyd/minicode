import { EventEmitter } from 'node:events'
import { execFile, type ChildProcess } from 'node:child_process'
import readline from 'node:readline'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import type { DesktopEvent } from './types.js'

export type BridgeApi = {
  request: <T = unknown>(method: string, params?: Record<string, unknown>) => Promise<T>
  onEvent: (listener: (event: DesktopEvent) => void) => () => void
  close: () => void | Promise<void>
}

const EVENTS = new Set(['run_started', 'agent_event', 'text_delta', 'reasoning_delta', 'approval', 'approval_auto', 'run_done', 'run_error', 'bridge_error', 'log', 'warning'])
// Bridge protocol versions this TUI understands.
export const PROTOCOL = { min: 2, max: 2 }

export type ProtocolProblem = { message: string; level: 'error' | 'warning' }

// Compares the agent's handshake with PROTOCOL. A mismatch is reported as an
// upgrade hint; the connection itself stays usable.
export function protocolProblem(initial: { protocolVersion?: number; minClientProtocol?: number; state?: { protocolVersion?: number } }): ProtocolProblem | null {
  const version = initial.protocolVersion ?? initial.state?.protocolVersion
  if (typeof version !== 'number') return null
  if (version < PROTOCOL.min) {
    return { level: 'error', message: `Agent 协议版本 ${version} 低于 TUI 需要的 ${PROTOCOL.min}，部分功能不可用；请升级 MiniCode agent。` }
  }
  if ((initial.minClientProtocol ?? 0) > PROTOCOL.max) {
    return { level: 'error', message: `Agent 需要协议版本 ${initial.minClientProtocol} 以上的客户端，当前 TUI 为 ${PROTOCOL.max}；请升级 TUI。` }
  }
  if (version > PROTOCOL.max) {
    return { level: 'warning', message: `Agent 协议版本 ${version} 比 TUI 支持的 ${PROTOCOL.max} 新，未知事件会被忽略；请升级 TUI。` }
  }
  return null
}

export class BridgeClient implements BridgeApi {
  private nextId = 1
  private pending = new Map<number, { resolve: (value: unknown) => void; reject: (error: Error) => void; timer: ReturnType<typeof setTimeout> }>()
  private events = new EventEmitter()
  private closed = false
  private failure: Error | null = null
  private closePromise?: Promise<void>
  private reader: readline.Interface
  private diagnostics?: fs.WriteStream
  private diagnosticsClosed = false
  /** Event types from a newer agent that this TUI ignored. */
  readonly ignoredEvents = new Set<string>()

  constructor(private readonly child: ChildProcess, private readonly requestTimeoutMs = 35000, private readonly logPath = process.env.MINICODE_BRIDGE_LOG || path.join(os.homedir(), '.minicode', 'logs', 'agent-stderr.log')) {
    if (!child.stdout || !child.stdin) throw new Error('Agent bridge requires piped stdio')
    this.reader = readline.createInterface({ input: child.stdout })
    this.reader.on('line', line => this.onLine(line))
    this.reader.on('close', () => this.fail(new Error('Agent bridge 输出通道已关闭')))
    child.stdout.on('error', error => this.fail(error))
    child.stderr?.on('data', chunk => {
      this.logDiagnostic(String(chunk))
    })
    child.on('exit', (code, signal) => this.fail(new Error(`Agent bridge exited (${signal || code})`)))
    child.on('error', error => this.fail(error))
    child.stdin.on('error', error => this.fail(error))
  }

  request<T = unknown>(method: string, params: Record<string, unknown> = {}): Promise<T> {
    if (this.failure) return Promise.reject(this.failure)
    if (this.closed || !this.child.stdin || this.child.exitCode !== null || this.child.signalCode || this.child.stdin.destroyed) {
      return Promise.reject(new Error('Agent bridge 不可用'))
    }
    const id = this.nextId++
    return new Promise((resolve, reject) => {
      try {
        const line = JSON.stringify({ id, method, params }) + '\n'
        if (Buffer.byteLength(line, 'utf8') > 4_000_000) throw new Error('request exceeds the size limit')
        const timer = setTimeout(() => {
          this.pending.delete(id)
          reject(new Error(`Agent bridge 请求超时：${method}。请检查会话状态后重试。`))
        }, this.requestTimeoutMs)
        this.pending.set(id, { resolve: value => resolve(value as T), reject, timer })
        this.child.stdin!.write(line, error => {
          if (error) this.fail(error)
        })
      } catch (error) {
        const pending = this.pending.get(id)
        if (pending) clearTimeout(pending.timer)
        this.pending.delete(id)
        reject(error instanceof Error ? error : new Error(String(error)))
      }
    })
  }

  onEvent(listener: (event: DesktopEvent) => void): () => void {
    this.events.on('event', listener)
    // A spawn failure can arrive before React mounts its event subscription.
    if (this.failure) queueMicrotask(() => {
      if (!this.closed && this.events.listeners('event').includes(listener)) {
        listener({ event: 'bridge_error', error: this.failure!.message, fatal: true })
      }
    })
    return () => { this.events.off('event', listener) }
  }

  close(): Promise<void> {
    if (this.closePromise) return this.closePromise
    this.closed = true
    this.rejectAll(new Error('Agent bridge 已关闭'))
    this.closePromise = this.stop().finally(() => this.closeDiagnostics())
    return this.closePromise
  }

  private logDiagnostic(text: string): void {
    if (this.diagnosticsClosed) return
    if (!this.diagnostics) {
      try {
        fs.mkdirSync(path.dirname(this.logPath), { recursive: true })
        this.diagnostics = fs.createWriteStream(this.logPath, { flags: 'a', mode: 0o600 })
        // Diagnostics must never break the bridge, including unwritable logs.
        this.diagnostics.on('error', () => {})
      } catch { return }
    }
    if (!this.diagnostics.destroyed) this.diagnostics.write(text)
  }

  private async closeDiagnostics(): Promise<void> {
    this.diagnosticsClosed = true
    const stream = this.diagnostics
    if (!stream || stream.closed) return
    await new Promise<void>(resolve => {
      stream.once('close', resolve)
      stream.end()
    })
  }

  private async stop(): Promise<void> {
    if (!this.child.pid || this.child.exitCode !== null || this.child.signalCode) {
      this.reader.close()
      return
    }
    let timer: ReturnType<typeof setTimeout> | undefined
    let onExit: () => void = () => {}
    const exited = new Promise<boolean>(resolve => {
      onExit = () => resolve(true)
      this.child.once('exit', onExit)
    })
    const deadline = new Promise<boolean>(resolve => { timer = setTimeout(() => resolve(false), 3000) })
    try {
      // EOF lets the agent cancel the run, persist checkpoints and close tool/MCP
      // processes before its loop exits. Force termination is only a fallback.
      try { this.child.stdin?.end() } catch { /* deadline still terminates a broken pipe's owner */ }
      if (!await Promise.race([exited, deadline])) {
        if (process.platform === 'win32') {
          await new Promise<void>(resolve => {
            execFile('taskkill', ['/PID', String(this.child.pid), '/T', '/F'], { windowsHide: true, timeout: 3000 }, () => resolve())
          })
        } else {
          try { process.kill(-this.child.pid, 'SIGKILL') } catch { this.child.kill('SIGKILL') }
        }
      }
    } finally {
      clearTimeout(timer)
      this.child.off('exit', onExit)
      this.reader.close()
    }
  }

  private onLine(line: string): void {
    if (this.closed || this.failure) return
    let message: Record<string, unknown>
    try {
      message = JSON.parse(line)
      if (!message || typeof message !== 'object' || Array.isArray(message)) throw new Error('消息不是对象')
      if ('event' in message) {
        if (typeof message.event !== 'string') throw new Error('事件类型无效')
        if (['log', 'warning'].includes(message.event) && typeof message.text !== 'string') throw new Error('日志消息无效')
        if (message.event === 'agent_event' && (!message.item || typeof message.item !== 'object')) throw new Error('缺少 agent event')
      } else if (!Number.isSafeInteger(message.id) || (!('result' in message) && !('error' in message))) {
        throw new Error('缺少请求 ID 或结果')
      }
    } catch (error) {
      this.fail(new Error(`Agent bridge 返回了无效消息: ${error}`))
      return
    }
    if (message.event) {
      // A newer agent may send events this TUI does not know: ignore them.
      if (!EVENTS.has(message.event as string)) {
        if (!this.ignoredEvents.has(message.event as string) && process.env.MINICODE_TUI_DEBUG) {
          process.stderr.write(`minicode tui: ignored unknown bridge event ${message.event}\n`)
        }
        this.ignoredEvents.add(message.event as string)
        return
      }
      if (message.event === 'log') { this.logDiagnostic(`[log] ${message.text}\n`); return }
      this.events.emit('event', message)
      return
    }
    const pending = this.pending.get(message.id as number)
    if (!pending) return
    this.pending.delete(message.id as number)
    clearTimeout(pending.timer)
    if ('error' in message) pending.reject(new Error(String(message.error)))
    else pending.resolve(message.result)
  }

  private fail(error: Error): void {
    if (this.closed || this.failure) return
    this.failure = error
    this.rejectAll(error)
    this.events.emit('event', { event: 'bridge_error', error: error.message, fatal: true })
  }

  private rejectAll(error: Error): void {
    for (const request of this.pending.values()) { clearTimeout(request.timer); request.reject(error) }
    this.pending.clear()
  }
}
