import assert from 'node:assert/strict'
import { PassThrough } from 'node:stream'
import { setTimeout as delay } from 'node:timers/promises'
import test from 'node:test'
import React from 'react'
import { displayWidth } from '../src/format.js'
import { FakeBridge } from './fake-bridge.js'

// This isolated test process must exercise Ink's interactive path on CI too.
process.env.CI = 'false'
process.env.CONTINUOUS_INTEGRATION = 'false'
const { render } = await import('ink')
const { App } = await import('../src/App.js')

// Track the actual ANSI cursor movements, including the terminal's bottom
// boundary. ink-testing-library's debug frames bypass this rendering path.
class TerminalOutput extends PassThrough {
  isTTY = true
  columns = 80
  rows = 24
  output = ''

  constructor() {
    super()
    this.on('data', chunk => { this.output += String(chunk) })
  }

  cursor() {
    let x = 0
    let y = 0
    let visible = true
    const tokens = this.output.match(/\x1b\[[0-?]*[ -/]*[@-~]|[\s\S]/gu) || []
    for (const token of tokens) {
      if (token.startsWith('\x1b[')) {
        const command = token.at(-1)
        const parameters = token.slice(2, -1)
        if (parameters.startsWith('?')) {
          if (parameters === '?25') visible = command === 'h'
          continue
        }
        const values = parameters.split(';').map(value => Number(value) || 1)
        const amount = values[0]
        if (command === 'H' || command === 'f') { y = amount - 1; x = (values[1] || 1) - 1 }
        if (command === 'A') y = Math.max(0, y - amount)
        if (command === 'B') y = Math.min(this.rows - 1, y + amount)
        if (command === 'C') x = Math.min(this.columns - 1, x + amount)
        if (command === 'D') x = Math.max(0, x - amount)
        if (command === 'G') x = amount - 1
        if (command === 'E') { y = Math.min(this.rows - 1, y + amount); x = 0 }
        if (command === 'F') { y = Math.max(0, y - amount); x = 0 }
      } else if (token === '\n') {
        y = Math.min(this.rows - 1, y + 1)
        x = 0
      } else if (token === '\r') {
        x = 0
      } else if (token >= ' ') {
        const width = displayWidth(token)
        if (x + width > this.columns) { y = Math.min(this.rows - 1, y + 1); x = 0 }
        x += width
      }
    }
    return { x, y, visible }
  }
}

async function waitFor(check: () => boolean, description: string) {
  for (let attempt = 0; attempt < 200; attempt++) {
    if (check()) return
    await delay(10)
  }
  assert.ok(check(), description)
}

async function expectCursor(stdout: TerminalOutput, x: number, y: number) {
  await waitFor(() => {
    const cursor = stdout.cursor()
    return cursor.x === x && cursor.y === y && cursor.visible
  }, `cursor reaches (${x}, ${y}) after rendering`)
  assert.deepEqual(stdout.cursor(), { x, y, visible: true })
}

async function mount() {
  const stdout = new TerminalOutput()
  const stdin = Object.assign(new PassThrough(), {
    isTTY: true,
    setRawMode() {},
    ref() {},
    unref() {},
  })
  const client = new FakeBridge()
  const ui = render(<App client={client} options={{ workspace: 'D:/workspace' }} />, {
    stdout: stdout as unknown as NodeJS.WriteStream,
    stdin: stdin as unknown as NodeJS.ReadStream,
    stderr: new PassThrough() as NodeJS.WriteStream,
    exitOnCtrlC: false,
    patchConsole: false,
    maxFps: 120,
  })
  await waitFor(() => client.requests.some(item => item.method === 'getCapabilities')
    && stdout.output.includes('输入任务'), 'interactive renderer initializes')
  await expectCursor(stdout, 3, 21)
  return { stdout, stdin, ui, client }
}

test('fullscreen native cursor stays on the input line for CJK text and caret movement without a second drawn cursor', async () => {
  const { stdout, stdin, ui } = await mount()
  try {
    await expectCursor(stdout, 3, 21)
    stdin.write('你好')
    await expectCursor(stdout, 7, 21)
    assert.doesNotMatch(stdout.output, /\x1b\[7m/, 'the prompt does not draw an inverse cursor')
    stdin.write('\x1b[D')
    await expectCursor(stdout, 5, 21)
    stdin.write('\x1b[C')
    await expectCursor(stdout, 7, 21)
  } finally {
    ui.unmount()
    ui.cleanup()
  }
})

test('native cursor follows multiline editing, wrapping and terminal resize', async () => {
  const { stdout, stdin, ui } = await mount()
  try {
    stdin.write('你好')
    stdin.write('\n')
    stdin.write('abc')
    await expectCursor(stdout, 6, 21)
    stdin.write('\x01')
    await expectCursor(stdout, 3, 20)
    stdin.write('\x1b[F')
    await expectCursor(stdout, 7, 20)
    stdout.columns = 10
    stdout.rows = 18
    stdout.emit('resize')
    await expectCursor(stdout, 7, 14)
    stdin.write('中文')
    await expectCursor(stdout, 5, 14)
  } finally {
    ui.unmount()
    ui.cleanup()
  }
})

test('native cursor remains on the draft when the history return button appears and disappears', async () => {
  const { stdout, stdin, ui, client } = await mount()
  try {
    stdin.write('history\r')
    await delay(40)
    client.emit({
      event: 'agent_event', sessionId: 'sess-1',
      item: { seq: 1, type: 'assistant_message', timestamp: new Date().toISOString(),
        data: { text: Array.from({ length: 80 }, (_, index) => `HISTORY ${index}`).join('\n\n') } },
    })
    client.emit({ event: 'run_done', sessionId: 'sess-1', result: { exit_reason: 'completed' } })
    await waitFor(() => stdout.output.includes('HISTORY 79'), 'history renders before editing')
    stdin.write('你好')
    await expectCursor(stdout, 7, 21)
    stdin.write('\x1b[<64;10;8M')
    await waitFor(() => stdout.output.includes('返回最新'), 'scroll button renders')
    assert.match(stdout.output, /返回最新/)
    await expectCursor(stdout, 7, 21)
    stdin.write('\x1b[<0;40;20M')
    await expectCursor(stdout, 7, 21)
    stdin.write('\r')
    await waitFor(() => client.requests.filter(item => item.method === 'sendPrompt').length === 2, 'edited prompt submits')
    assert.equal(client.requests.filter(item => item.method === 'sendPrompt')[1]?.params.text, '你好')
  } finally {
    ui.unmount()
    ui.cleanup()
  }
})
