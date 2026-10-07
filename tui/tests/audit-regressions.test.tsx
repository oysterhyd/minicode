import assert from 'node:assert/strict'
import { EventEmitter } from 'node:events'
import { PassThrough } from 'node:stream'
import { spawn, type ChildProcess } from 'node:child_process'
import { join as pathJoin } from 'node:path'
import { setTimeout as delay } from 'node:timers/promises'
import { afterEach, test } from 'node:test'
import React from 'react'
import { cleanup, render } from 'ink-testing-library'
import { App } from '../src/App.js'
import { BridgeClient, type BridgeApi } from '../src/bridge.js'
import { parseArgs } from '../src/args.js'
import { deleteBackward, deleteForward, moveVertical, promptRows } from '../src/caret.js'
import { applyDesktopEvent, emptyFeed } from '../src/feed.js'
import { displayWidth, truncateWidth } from '../src/format.js'
import stripAnsi from 'strip-ansi'
import { MarkdownView } from '../src/markdown.js'
import { slashSuggestions } from '../src/slash.js'
import { bridgeLaunch } from '../src/launch.js'
import { layoutTranscript } from '../src/transcript.js'
import { emptyState, historyItems, type AgentState, type DesktopEvent, type FeedItem } from '../src/types.js'

afterEach(cleanup)

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>(done => { resolve = done })
  return { promise, resolve }
}

async function eventually(check: () => boolean, message: string) {
  for (let attempt = 0; attempt < 120; attempt++) {
    if (check()) return
    await delay(10)
  }
  assert.ok(check(), message)
}

// Independent protocol simulator: no fixtures or assertions from the existing suite.
class ProbeBridge implements BridgeApi {
  calls: Array<{ method: string; params: Record<string, unknown> }> = []
  listeners = new Set<(event: DesktopEvent) => void>()
  state: AgentState = { ...emptyState, model: 'fake' }
  heldSend?: ReturnType<typeof deferred<{ sessionId: string }>>
  heldInitialize?: ReturnType<typeof deferred<unknown>>
  heldState?: ReturnType<typeof deferred<AgentState>>
  heldSlash?: ReturnType<typeof deferred<void>>
  continuationCompletesEarly = false
  foreignWorkspace = 'D:/other-project'
  sessions = [{ session_id: 'restored-session', title: 'restored' }]
  async request<T>(method: string, params: Record<string, unknown> = {}): Promise<T> {
    this.calls.push({ method, params })
    let result: unknown
    if (method === 'initialize') result = this.heldInitialize ? await this.heldInitialize.promise : {
      state: this.state, defaultModel: 'fake', models: [], sessions: this.sessions,
      commands: ['/help', '/new', '/resume', '/continue', '/clear', '/exit', '/model', '/permissions', '/skill'].map(name => ({ name, usage: name, summary: name })),
    }
    else if (method === 'getCapabilities') result = { skills: [] }
    else if (method === 'listSessions') result = this.sessions
    else if (method === 'getState') result = this.heldState ? await this.heldState.promise : { ...this.state }
    else if (method === 'sendPrompt') {
      this.state = { ...this.state, sessionId: 'active', taskPending: true, running: true }
      result = this.heldSend ? await this.heldSend.promise : { sessionId: 'active' }
    } else if (method === 'runSlash') {
      if (this.heldSlash) await this.heldSlash.promise
      const text = String(params.text)
      if (text.startsWith('/resume')) result = { action: 'resume', sessionId: 'restored-session' }
      else if (text === '/new') {
        this.state = { ...emptyState, model: 'fake' }
        result = { action: 'new' }
      } else if (text === '/continue') {
        if (this.continuationCompletesEarly) this.emit({ event: 'run_done', sessionId: 'active', result: { exit_reason: 'completed' } })
        result = { action: 'continue', state: { ...this.state } }
      } else if (text === '/clear') result = { action: 'clear' }
      else if (text === '/exit') result = { action: 'exit' }
      else result = { message: 'ok', state: { ...this.state } }
    } else if (method === 'getSession') result = {
      summary: { session_id: 'restored-session', workspace: this.foreignWorkspace, model: 'fake' }, messages: [], events: [],
    }
    else if (method === 'selectSession') result = this.state = { ...this.state, sessionId: 'restored-session' }
    else if (method === 'cancelTurn') result = true
    else if (method === 'resolveApproval') result = true
    else result = { ...this.state }
    return result as T
  }
  onEvent(listener: (event: DesktopEvent) => void) { this.listeners.add(listener); return () => { this.listeners.delete(listener) } }
  close() {}
  emit(event: DesktopEvent) { for (const listener of this.listeners) listener(event) }
  prompts() { return this.calls.filter(call => call.method === 'sendPrompt') }
}

async function mount(client = new ProbeBridge()) {
  const ui = render(<App client={client} options={{ workspace: 'D:/workspace' }} />)
  await eventually(() => client.calls.some(call => call.method === 'getCapabilities'), 'TUI initializes')
  await delay(30)
  return { client, ui }
}
function enter(ui: ReturnType<typeof render>, text: string) { ui.stdin.write(text); ui.stdin.write('\r') }
function agent(seq: number, type: string, data: Record<string, unknown>): DesktopEvent {
  return { event: 'agent_event', sessionId: 'active', item: { seq, type, data, timestamp: '2026-10-08T10:00:00Z' } }
}

test('rapid submissions keep a FIFO queue, including completion before the send response', async () => {
  const { client, ui } = await mount()
  client.heldSend = deferred()
  enter(ui, 'first')
  enter(ui, 'second')
  enter(ui, 'third')
  await eventually(() => client.prompts().length === 1, 'only one activation starts')
  client.emit({ event: 'run_done', sessionId: 'active', result: { exit_reason: 'completed' } })
  client.heldSend.resolve({ sessionId: 'active' })
  client.heldSend = undefined
  await eventually(() => client.prompts().length === 2, 'second prompt drains after the response')
  client.emit({ event: 'run_done', sessionId: 'active', result: { exit_reason: 'completed' } })
  await eventually(() => client.prompts().length === 3, 'third prompt was preserved')
  assert.deepEqual(client.prompts().map(call => call.params.text), ['first', 'second', 'third'])
})

test('cancelled and paused runs retain queued input without starting another turn', async () => {
  for (const reason of ['cancelled', 'max_rounds', 'provider_error']) {
    const { client, ui } = await mount()
    enter(ui, 'first')
    await eventually(() => client.prompts().length === 1, 'run starts')
    enter(ui, 'next')
    client.emit({ event: 'run_done', sessionId: 'active', result: { exit_reason: reason } })
    await delay(50)
    assert.equal(client.prompts().length, 1)
    assert.match(ui.lastFrame() || '', /已排队 1 条/)
    ui.stdin.write('\r')
    await eventually(() => client.prompts().length === 2, 'Enter explicitly sends retained input')
    ui.unmount()
  }
})

test('forward Delete and terminal Backspace remove different graphemes', async () => {
  const { client, ui } = await mount()
  ui.stdin.write('a中👩‍💻b')
  ui.stdin.write('\x1b[H')
  ui.stdin.write('\x1b[C')
  ui.stdin.write('\x1b[3~')
  ui.stdin.write('\x1b[C')
  ui.stdin.write('\x7f')
  ui.stdin.write('\r')
  await eventually(() => client.prompts().length === 1, 'edited prompt submitted')
  assert.equal(client.prompts()[0].params.text, 'ab')
})

test('bracketed multiline paste inserts text without executing commands or control keys', async () => {
  const { client, ui } = await mount()
  ui.stdin.write('\x1b[200~')
  ui.stdin.write('/new\r\nhello\tworld')
  ui.stdin.write('\x1b[201~')
  await delay(30)
  assert.equal(client.calls.some(call => call.method === 'runSlash'), false)
  assert.match(ui.lastFrame() || '', /hello  world/)
  ui.stdin.write('\x03') // clear the draft
  assert.equal(client.calls.some(call => call.method === 'runSlash'), false)
})

test('Tab completes without execution and exact free-form slash arguments are preserved', async () => {
  const { client, ui } = await mount()
  ui.stdin.write('/he')
  ui.stdin.write('\t')
  await delay(25)
  assert.equal(client.calls.some(call => call.method === 'runSlash'), false)
  ui.stdin.write('\r')
  await eventually(() => client.calls.some(call => call.method === 'runSlash'), 'completed slash executes on Enter')
  assert.equal(client.calls.find(call => call.method === 'runSlash')?.params.text, '/help')
})

test('resume switches to the stored workspace; new clears the session state', async () => {
  const { client, ui } = await mount()
  enter(ui, '/resume restored-session')
  await eventually(() => client.calls.some(call => call.method === 'selectSession'), 'session selected')
  await delay(30)
  enter(ui, 'follow-up')
  await eventually(() => client.prompts().length === 1, 'follow-up starts')
  assert.equal(client.prompts()[0].params.workspace, client.foreignWorkspace)
  assert.equal(client.prompts()[0].params.sessionId, 'restored-session')
  client.emit({ event: 'run_done', sessionId: 'active', result: { exit_reason: 'completed' } })
  await delay(25)
  enter(ui, '/new')
  await delay(40)
  enter(ui, 'fresh')
  await eventually(() => client.prompts().length === 2, 'new session starts')
  assert.equal(client.prompts()[1].params.sessionId, null)
})

test('continue cannot stay busy if its completion arrives before the RPC response', async () => {
  const { client, ui } = await mount()
  enter(ui, 'first')
  await eventually(() => client.prompts().length === 1, 'first starts')
  client.emit({ event: 'run_done', sessionId: 'active', result: { exit_reason: 'max_rounds' } })
  client.continuationCompletesEarly = true
  enter(ui, '/continue')
  await delay(50)
  enter(ui, 'after continue')
  await eventually(() => client.prompts().length === 2, 'no phantom busy state after continue')
})

test('stale state refresh cannot overwrite a newer selected session', async () => {
  const { client, ui } = await mount()
  enter(ui, 'first')
  await eventually(() => client.prompts().length === 1, 'first starts')
  client.heldState = deferred()
  client.emit({ event: 'run_done', sessionId: 'active', result: { exit_reason: 'completed' } })
  enter(ui, '/resume restored-session')
  await delay(40)
  client.heldState.resolve({ ...emptyState, sessionId: 'active', model: 'stale-model' })
  client.heldState = undefined
  await delay(40)
  assert.doesNotMatch(ui.lastFrame() || '', /stale-model/)
})

test('approval hotkeys never double-resolve the same request', async () => {
  const { client, ui } = await mount()
  enter(ui, 'first')
  await eventually(() => client.prompts().length === 1, 'first starts')
  client.emit(agent(1, 'approval_request', { call_id: 'tool-1' }))
  client.emit({ event: 'approval', sessionId: 'active', approvalId: 'approval-1', request: { tool_name: 'write', arguments: { path: 'x' }, summary: 'write' } })
  ui.stdin.write('y')
  ui.stdin.write('y')
  await delay(30)
  assert.equal(client.calls.filter(call => call.method === 'resolveApproval').length, 1)
})

test('Ctrl+C can exit during initialization and two rapid presses exit after initialization', async () => {
  const blocked = new ProbeBridge()
  blocked.heldInitialize = deferred()
  const connecting = render(<App client={blocked} options={{ workspace: '.' }} />)
  await delay(25)
  connecting.stdin.write('\x03')
  await delay(25)
  const before = connecting.frames.length
  connecting.stdin.write('ignored')
  await delay(25)
  assert.equal(connecting.frames.length, before)
  const { ui } = await mount()
  ui.stdin.write('\x03')
  ui.stdin.write('\x03')
  await delay(25)
  const after = ui.frames.length
  ui.stdin.write('ignored')
  await delay(25)
  assert.equal(ui.frames.length, after)
})

test('transcript never freezes a mutable quiet group or rows after pending parallel tools', () => {
  const read: FeedItem = { id: 'read-1', kind: 'tool', name: 'read', pending: true }
  const done: FeedItem = { id: 'write-1', kind: 'tool', name: 'write', success: true }
  let rows = layoutTranscript([read, done], false)
  assert.equal(rows.frozen.length, 0)
  assert.deepEqual(rows.live.map(row => row.id), ['read-1', 'write-1'])
  rows = layoutTranscript([{ ...read, pending: false, success: true }], false)
  assert.equal(rows.frozen.length, 0)
  rows = layoutTranscript([{ ...read, pending: false, success: true }, done], false)
  assert.equal(rows.frozen[0].kind, 'thought')
  assert.equal(rows.frozen[1].id, 'write-1')
})

test('streamed Markdown stays intact and final text updates in place across notices', () => {
  let feed = { ...emptyFeed(), sessionId: 'active', busy: true }
  feed = applyDesktopEvent(feed, { event: 'text_delta', sessionId: 'active', text: 'Intro\n\n[link][ref]' })
  feed = { ...feed, items: [...feed.items, { id: 'notice', kind: 'notice', text: 'notice' }] }
  feed = applyDesktopEvent(feed, { event: 'text_delta', sessionId: 'active', text: '\n\n[ref]: https://example.com' })
  assert.equal(layoutTranscript(feed.items, false).frozen.length, 0)
  feed = applyDesktopEvent(feed, agent(5, 'assistant_message', { text: 'Intro\n\n[link][ref]\n\n[ref]: https://example.com' }))
  assert.deepEqual(feed.items.map(item => item.kind), ['assistant', 'notice'])
  assert.match(feed.items[0].text || '', /\[ref\]: https/)
})

test('foreign events are ignored, retries discard partial text, fatal errors seal pending tools', () => {
  const start = { ...emptyFeed(), sessionId: 'active', busy: true, startedAt: Date.now() }
  assert.equal(applyDesktopEvent(start, { event: 'run_done', sessionId: 'foreign', result: { exit_reason: 'completed' } }), start)
  let feed = applyDesktopEvent(start, { event: 'text_delta', sessionId: 'active', text: 'partial' })
  feed = applyDesktopEvent(feed, agent(2, 'provider_retry', { delay_s: 1 }))
  assert.equal(feed.items.some(item => item.kind === 'assistant'), false)
  feed = applyDesktopEvent(feed, agent(3, 'tool_call_start', { call_id: 'read', name: 'read' }))
  feed = applyDesktopEvent(feed, { event: 'bridge_error', error: 'spawn ENOENT', fatal: true })
  assert.equal(feed.busy, false)
  assert.equal(feed.startedAt, null)
  assert.equal(feed.items.find(item => item.kind === 'tool')?.interrupted, true)
})

test('a recovered call does not mutate an already printed interrupted tool', () => {
  let feed = applyDesktopEvent({ ...emptyFeed(), sessionId: 'active' }, agent(1, 'tool_call_start', { call_id: 'x', name: 'write' }))
  feed = applyDesktopEvent(feed, { event: 'run_done', sessionId: 'active', result: { exit_reason: 'cancelled' } })
  const printed = feed.items[0]
  feed = applyDesktopEvent(feed, agent(2, 'tool_call_start', { call_id: 'x', name: 'write', recovered: true }))
  feed = applyDesktopEvent(feed, agent(3, 'tool_call_result', { call_id: 'x', success: true, output_detail: 'done' }))
  assert.equal(feed.items[0], printed)
  assert.equal(feed.items[1].success, true)
  assert.notEqual(feed.items[0].id, feed.items[1].id)
})

test('failed quiet tools and failed skill loads retain their actual failure output', async () => {
  const failed: FeedItem = { id: 'r', kind: 'tool', name: 'read', success: false, output: 'permission denied' }
  assert.equal(layoutTranscript([failed]).frozen[0].kind, 'tool')
  const { client, ui } = await mount()
  enter(ui, 'first')
  await eventually(() => client.prompts().length === 1, 'first starts')
  client.emit(agent(1, 'tool_call_start', { call_id: 'skill', name: 'skill_load', arguments: { name: 'missing' } }))
  client.emit(agent(2, 'tool_call_result', { call_id: 'skill', success: false, error: 'skill unavailable' }))
  await delay(30)
  assert.match(ui.lastFrame() || '', /skill unavailable/)
  assert.doesNotMatch(ui.lastFrame() || '', /Successfully loaded skill/)
})

test('Unicode editing and wrapping agree with terminal cell width', () => {
  assert.equal(displayWidth('👩‍💻中é'), 5)
  assert.equal(deleteBackward('a👩‍💻b', 6).text, 'ab')
  assert.equal(deleteForward('aéb', 1).text, 'ab')
  assert.equal(moveVertical('a中b\n👩‍💻xy', 2, 1), 10)
  assert.equal(truncateWidth('中文abc', 1), '…')
  const layout = promptRows('abcd中文👩‍💻', 11, 4)
  assert.ok(layout.rows.every(row => displayWidth(row.text) <= 4))
  assert.ok(displayWidth(layout.rows[layout.row].text.slice(0, layout.column)) < 4)
})

test('history recovers tool-result messages without events and marks unexecuted calls', () => {
  const items = historyItems({ summary: { session_id: 'x', workspace: '.', model: 'fake' }, events: [], messages: [
    { role: 'assistant', content: [{ type: 'tool_use', id: 'a', name: 'read', input: {} }, { type: 'tool_use', id: 'b', name: 'write', input: {} }] },
    { role: 'user', content: [{ type: 'tool_result', tool_use_id: 'a', content: 'persisted error', is_error: true }] },
  ] })
  assert.equal(items[0].output, 'persisted error')
  assert.equal(items[0].success, false)
  assert.equal(items[1].interrupted, true)
})

test('numeric options reject NaN, infinite and fractional budgets; script implies fake at launch', () => {
  for (const argv of [['--max-rounds', 'NaN'], ['--max-rounds', '1.5'], ['--max-seconds', 'Infinity'], ['--max-seconds', '-1'], ['--workspace', '--yes'], ['--provider', 'invalid']]) {
    assert.throws(() => parseArgs(argv))
  }
  assert.equal(parseArgs(['--max-seconds', '0.5']).maxSeconds, 0.5)
})

test('resume completion preserves full session IDs and skill off arguments', () => {
  const context = { commands: [], models: [], skills: [{ name: 'review', description: 'review' }], sessions: [{ session_id: '12345678-aaaa' }, { session_id: '12345678-bbbb' }] }
  assert.deepEqual(slashSuggestions('/resume ', context).map(item => item.value), ['12345678-aaaa', '12345678-bbbb'])
  assert.equal(slashSuggestions('/skill off rev', context)[0].value, 'off review')
})

test('Markdown preserves code literals, nested quotes, block code in lists, and ordered-list numbering', async () => {
  const ui = render(<MarkdownView text={'```sh\necho $HOME $PATH\n```\n\n`$x$` and $5 and $10\n\n> - nested quote\n\n9. first\n10. second\n\n- code:\n\n  ```js\n  const ok = true\n  ```'} />)
  await delay(30)
  const frame = ui.lastFrame() || ''
  assert.match(frame, /echo \$HOME \$PATH/)
  assert.match(frame, /\$x\$/)
  assert.match(frame, /\$5 and \$10/)
  assert.match(frame, /nested quote/)
  assert.match(frame, /10\. second/)
  assert.match(frame, /const ok = true/)
})

test('bridge marks invalid NDJSON and spawn errors terminal for present and future requests', async () => {
  const child = new EventEmitter() as ChildProcess
  Object.assign(child, { stdin: new PassThrough(), stdout: new PassThrough(), stderr: new PassThrough(), exitCode: null, signalCode: null, kill() { return true } })
  const client = new BridgeClient(child)
  const pending = client.request('initialize')
  const rejected = assert.rejects(pending, /无效消息/)
  child.stdout!.emit('data', 'not-json\n')
  await rejected
  await assert.rejects(client.request('getState'), /无效消息/)
  await client.close()
  const missing = spawn('minicode-intentionally-missing-python-73485', [], { stdio: 'pipe' })
  const failed = new BridgeClient(missing)
  await assert.rejects(failed.request('initialize'), /ENOENT/)
  await assert.rejects(failed.request('getState'), /ENOENT/)
  await failed.close()
})

test('bridge shutdown uses EOF so Python can clean up before exit', async () => {
  const child = spawn(process.execPath, ['-e', "process.stdin.resume(); process.stdin.on('end', () => process.exit(0))"], { stdio: 'pipe' })
  const client = new BridgeClient(child)
  await client.close()
  assert.equal(child.exitCode, 0)
  assert.equal(child.signalCode, null)
})

test('clearing discards the old transcript from Ink cache replays', async () => {
  const { client, ui } = await mount()
  enter(ui, 'old prompt')
  await eventually(() => client.prompts().length === 1, 'run starts')
  client.emit(agent(1, 'assistant_message', { text: 'OLD TRANSCRIPT SENTINEL' }))
  client.emit({ event: 'run_done', sessionId: 'active', result: { exit_reason: 'completed' } })
  enter(ui, '/clear')
  await delay(40)
  const replay = ui.lastFrame() || ''
  assert.ok(ui.frames.some(frame => frame.includes('\x1b[2J\x1b[H')), 'terminal is cleared')
  assert.doesNotMatch(replay, /OLD TRANSCRIPT SENTINEL/)
  Object.defineProperty(ui.stdout, 'columns', { value: 40, writable: true })
  ui.stdout.emit('resize')
  await delay(30)
  assert.doesNotMatch(ui.lastFrame() || '', /OLD TRANSCRIPT SENTINEL/)
})

test('fullscreen layout fills terminal rows and keeps the prompt at the bottom after resize', async () => {
  const { ui } = await mount()
  const initial = stripAnsi(ui.lastFrame() || '').split('\n')
  assert.equal(initial.length, 24)
  assert.match(initial.slice(0, 15).join('\n'), /MiniCode/)
  assert.match(initial[21], /输入任务/)
  assert.match(initial[23], /fake/)

  for (const [columns, rows] of [[60, 30], [38, 18], [80, 8]]) {
    Object.defineProperty(ui.stdout, 'columns', { value: columns, writable: true, configurable: true })
    Object.defineProperty(ui.stdout, 'rows', { value: rows, writable: true, configurable: true })
    ui.stdout.emit('resize')
    await delay(40)
    const lines = stripAnsi(ui.lastFrame() || '').split('\n')
    assert.equal(lines.length, rows)
    assert.ok(lines.every(line => displayWidth(line) <= columns))
    assert.match(lines[rows - 3], /输入任务/)
    assert.match(lines[rows - 1], /fake/)
  }
})

test('narrow terminal wraps long CJK drafts and renders wide Markdown tables without overflowing', async () => {
  const { client, ui } = await mount()
  Object.defineProperty(ui.stdout, 'columns', { value: 38, writable: true })
  Object.defineProperty(ui.stdout, 'rows', { value: 18, writable: true })
  ui.stdout.emit('resize')
  ui.stdin.write('很长的中文输入'.repeat(10) + '👩‍💻')
  await delay(40)
  const prompt = stripAnsi(ui.lastFrame() || '')
  assert.equal(prompt.split('\n').length, 18)
  assert.ok(prompt.split('\n').every(line => displayWidth(line) <= 38))
  ui.stdin.write('\x03')
  enter(ui, 'table')
  await eventually(() => client.prompts().length === 1, 'table request starts')
  client.emit({ event: 'text_delta', sessionId: 'active', text: '| Feature | Description |\n|---|---|\n| TUI | 中文说明'.repeat(1) + '很长的说明'.repeat(10) + ' |' })
  await delay(40)
  const frame = stripAnsi(ui.lastFrame() || '')
  assert.ok(frame.slice(frame.indexOf('❯ table')).split('\n').every(line => displayWidth(line) <= 38), frame)
})

test('a cancellation requested before sendPrompt responds is applied again after session assignment', async () => {
  const { client, ui } = await mount()
  client.heldSend = deferred()
  enter(ui, 'slow startup')
  await eventually(() => client.prompts().length === 1, 'request is pending')
  ui.stdin.write('\x03')
  client.heldSend.resolve({ sessionId: 'active' })
  await eventually(() => client.calls.filter(call => call.method === 'cancelTurn').length === 2, 'cancel is retried after startup')
  assert.equal(client.calls.filter(call => call.method === 'cancelTurn')[1].params.sessionId, 'active')
})

test('coalesced PTY text and Enter are interpreted as submissions rather than pasted newlines', async () => {
  const { client, ui } = await mount()
  ui.stdin.write('first\rsecond\r')
  await eventually(() => client.prompts().length === 1, 'first prompt starts from one PTY read')
  client.emit({ event: 'run_done', sessionId: 'active', result: { exit_reason: 'completed' } })
  await eventually(() => client.prompts().length === 2, 'second prompt drains')
  assert.deepEqual(client.prompts().map(call => call.params.text), ['first', 'second'])
})

test('long replies keep their tail and the prompt inside the fullscreen viewport during and after streaming', async () => {
  const { client, ui } = await mount()
  Object.defineProperty(ui.stdout, 'columns', { value: 60, writable: true })
  Object.defineProperty(ui.stdout, 'rows', { value: 24, writable: true })
  ui.stdout.emit('resize')
  enter(ui, 'stream')
  await eventually(() => client.prompts().length === 1, 'stream starts')
  const text = Array.from({ length: 80 }, (_, index) => `STREAM LINE ${index}`).join('\n\n')
  client.emit({ event: 'text_delta', sessionId: 'active', text })
  await delay(40)
  const pending = stripAnsi(ui.lastFrame() || '')
  assert.match(pending, /STREAM LINE 79/)
  assert.doesNotMatch(pending, /STREAM LINE 0\n/)
  assert.match(pending, /输入下一条消息/)
  assert.equal(pending.split('\n').length, 24)
  client.emit(agent(1, 'assistant_message', { text }))
  client.emit({ event: 'run_done', sessionId: 'active', result: { exit_reason: 'completed' } })
  await delay(40)
  const completed = stripAnsi(ui.lastFrame() || '')
  assert.equal(completed.split('\n').length, 24)
  assert.doesNotMatch(completed, /STREAM LINE 0\n/)
  assert.match(completed, /STREAM LINE 79/)
  assert.match(completed, /输入任务/)
})

test('mouse wheel scrolls the full history and the centered return button restores the latest output without editing the draft', async () => {
  const { client, ui } = await mount()
  enter(ui, 'history')
  await eventually(() => client.prompts().length === 1, 'history request starts')
  const text = Array.from({ length: 80 }, (_, index) => `HISTORY ROW ${index}`).join('\n\n')
  client.emit(agent(1, 'assistant_message', { text }))
  client.emit({ event: 'run_done', sessionId: 'active', result: { exit_reason: 'completed' } })
  await delay(40)
  assert.match(ui.lastFrame() || '', /HISTORY ROW 79/)
  assert.doesNotMatch(ui.lastFrame() || '', /返回最新/)

  ui.stdin.write('draft preserved')
  ui.stdin.write('\x1b[<64;10;8M')
  await eventually(() => (ui.lastFrame() || '').includes('返回最新'), 'wheel scroll reveals the return button')
  for (let step = 0; step < 70; step++) ui.stdin.write('\x1b[<64;10;8M')
  await delay(40)
  const frame = stripAnsi(ui.lastFrame() || '')
  assert.match(frame, /HISTORY ROW 0\n/)
  assert.doesNotMatch(frame, /HISTORY ROW 79/)
  assert.match(frame, /draft preserved/)
  assert.doesNotMatch(frame, /<64;/)
  const lines = frame.split('\n')
  assert.equal(lines.length, 24)
  const row = lines.findIndex(line => line.includes('返回最新'))
  assert.equal(row, 19, 'button is immediately above the prompt border')
  const label = lines[row].trimEnd()
  const left = displayWidth(label) - displayWidth(label.trimStart())
  assert.ok(Math.abs(left - (100 - displayWidth(label.trim())) / 2) <= 1, 'button is centered')

  ui.stdin.write(`\x1b[<0;1;${row + 1}M`)
  ui.stdin.write(`\x1b[<0;1;${row + 1}m`)
  await delay(30)
  assert.match(ui.lastFrame() || '', /返回最新/, 'clicking outside the button keeps the reading position')
  const column = displayWidth(lines[row].slice(0, lines[row].indexOf('返回最新'))) + 1
  ui.stdin.write(`\x1b[<0;${column};${row + 1}M`)
  ui.stdin.write(`\x1b[<0;${column};${row + 1}m`)
  await eventually(() => !(ui.lastFrame() || '').includes('返回最新'), 'click returns to latest')
  assert.match(ui.lastFrame() || '', /HISTORY ROW 79/)
  assert.match(ui.lastFrame() || '', /draft preserved/)
  ui.stdin.write('\r')
  await eventually(() => client.prompts().length === 2, 'draft can still be submitted')
  assert.equal(client.prompts()[1].params.text, 'draft preserved')
})

test('reading history stays anchored while new text streams, and wheel down resumes following the latest output', async () => {
  const { client, ui } = await mount()
  enter(ui, 'stream while reading')
  await eventually(() => client.prompts().length === 1, 'stream starts')
  const text = Array.from({ length: 60 }, (_, index) => `ANCHORED ROW ${index}`).join('\n\n')
  client.emit({ event: 'text_delta', sessionId: 'active', text })
  await delay(40)
  ui.stdin.write('\x1b[5~')
  await eventually(() => (ui.lastFrame() || '').includes('返回最新'), 'PageUp scrolls history')
  const before = stripAnsi(ui.lastFrame() || '').split('\n').filter(line => line.includes('ANCHORED ROW'))
  client.emit({ event: 'text_delta', sessionId: 'active', text: '\n\nLATEST STREAM SENTINEL' })
  await delay(40)
  const after = stripAnsi(ui.lastFrame() || '').split('\n').filter(line => line.includes('ANCHORED ROW'))
  assert.deepEqual(after, before, 'incoming text does not move the reading position')
  assert.doesNotMatch(ui.lastFrame() || '', /LATEST STREAM SENTINEL/)
  for (let step = 0; step < 20; step++) ui.stdin.write('\x1b[<65;10;8M')
  await eventually(() => !(ui.lastFrame() || '').includes('返回最新'), 'wheel down reaches the bottom')
  assert.match(ui.lastFrame() || '', /LATEST STREAM SENTINEL/)
  client.emit({ event: 'text_delta', sessionId: 'active', text: '\n\nFOLLOWING STREAM SENTINEL' })
  await eventually(() => (ui.lastFrame() || '').includes('FOLLOWING STREAM SENTINEL'), 'following resumes automatically')
})

test('history remains accessible through resizing and clears its scroll position with a new screen', async () => {
  const { client, ui } = await mount()
  enter(ui, 'resize history')
  await eventually(() => client.prompts().length === 1, 'run starts')
  client.emit(agent(1, 'assistant_message', { text: Array.from({ length: 80 }, (_, index) => `RESIZE ROW ${index}`).join('\n\n') }))
  client.emit({ event: 'run_done', sessionId: 'active', result: { exit_reason: 'completed' } })
  await delay(40)
  ui.stdin.write('\x1b[1;5H')
  await eventually(() => (ui.lastFrame() || '').includes('RESIZE ROW 0'), 'Ctrl+Home reaches the oldest content')
  for (const [columns, rows] of [[38, 18], [80, 30]]) {
    Object.defineProperty(ui.stdout, 'columns', { value: columns, writable: true, configurable: true })
    Object.defineProperty(ui.stdout, 'rows', { value: rows, writable: true, configurable: true })
    ui.stdout.emit('resize')
    await delay(40)
    const lines = stripAnsi(ui.lastFrame() || '').split('\n')
    assert.equal(lines.length, rows)
    assert.ok(lines.every(line => displayWidth(line) <= columns))
    assert.match(lines[rows - 3], /输入任务/)
    assert.match(lines[rows - 1], /fake/)
  }
  ui.stdin.write('\x1b[6~')
  await delay(30)
  assert.match(ui.lastFrame() || '', /返回最新/)
  ui.stdin.write('\x1b[1;5F')
  await eventually(() => !(ui.lastFrame() || '').includes('返回最新'), 'Ctrl+End returns to latest')
  assert.match(ui.lastFrame() || '', /RESIZE ROW 79/)
  ui.stdin.write('\x1b[5~')
  await delay(30)
  enter(ui, '/clear')
  await eventually(() => !(ui.lastFrame() || '').includes('返回最新'), 'clear resets scroll state')
  assert.doesNotMatch(ui.lastFrame() || '', /RESIZE ROW/)
  assert.match(ui.lastFrame() || '', /MiniCode/)
})

test('Enter accepts a partial submenu value while preserving an explicitly typed command argument', async () => {
  const { client, ui } = await mount()
  enter(ui, '/resume resto')
  await eventually(() => client.calls.some(call => call.method === 'selectSession'), 'partial session ID completes')
  assert.equal(client.calls.find(call => call.method === 'runSlash')?.params.text, '/resume restored-session')
  await delay(30)
  enter(ui, '/skill off unlisted-name')
  await delay(30)
  assert.equal(client.calls.filter(call => call.method === 'runSlash').at(-1)?.params.text, '/skill off unlisted-name')
})

test('prompts submitted during a slow slash command drain after the command completes', async () => {
  const { client, ui } = await mount()
  client.heldSlash = deferred()
  enter(ui, '/help')
  enter(ui, 'queued after command')
  await delay(25)
  assert.equal(client.prompts().length, 0)
  client.heldSlash.resolve(undefined)
  await eventually(() => client.prompts().length === 1, 'queued input drains after slash response')
  assert.equal(client.prompts()[0].params.text, 'queued after command')
})

test('bundled launch uses the supplied environment resource path', () => {
  const resources = 'D:/minicode-audit-resources'
  const launch = bridgeLaunch({ environment: { MINICODE_RESOURCES: resources }, userData: 'D:/minicode-audit-data' })
  assert.equal(launch.file, pathJoin(resources, 'runtime', 'python', process.platform === 'win32' ? 'python.exe' : 'python'))
  assert.equal(launch.args.at(-1), pathJoin(resources, 'runtime', 'bridge.py'))
})

test('forced Windows shutdown terminates descendants when a bridge ignores EOF', { skip: process.platform !== 'win32', timeout: 12000 }, async () => {
  const source = 'const {spawn}=require("node:child_process"); const worker=spawn(process.execPath,["-e","setInterval(()=>{},1000)"],{stdio:"ignore"}); process.stdout.write(JSON.stringify({id:1,result:{pid:worker.pid}})+"\\n"); process.stdin.resume(); setInterval(()=>{},1000)'
  const child = spawn(process.execPath, ['-e', source], { stdio: 'pipe', windowsHide: true })
  const client = new BridgeClient(child)
  const { pid } = await client.request<{ pid: number }>('initialize')
  await client.close()
  await delay(40)
  assert.notEqual(child.exitCode, null)
  assert.throws(() => process.kill(pid, 0), /ESRCH/)
})
