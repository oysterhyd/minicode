import assert from 'node:assert/strict'
import test from 'node:test'
import React from 'react'
import { render } from 'ink-testing-library'
import stripAnsi from 'strip-ansi'
import { App } from '../src/App.js'
import { diffPairs, parseToolDiff, previewDiff, wordDiff } from '../src/tools.js'
import { displayWidth } from '../src/format.js'
import { delay, FakeBridge } from './fake-bridge.js'

const DIFF = `Applied change to src/App.tsx (+2 -1)
--- a/src/App.tsx
+++ b/src/App.tsx
@@ -11,3 +11,4 @@
 export function App() {
-  const [theme] = useState("dark")
+  const [theme, setTheme] = useState("system")
+  // Follow the system appearance
   return (`

test('edit output parses into numbered hunks with paired lines', () => {
  const diff = parseToolDiff(DIFF)
  assert.ok(diff)
  assert.equal(diff.path, 'src/App.tsx')
  assert.deepEqual([diff.adds, diff.dels], [2, 1])
  assert.deepEqual(diff.lines.map(line => `${line.line}${line.kind[0]}`), ['11c', '12d', '12a', '13a', '14c'])
  assert.deepEqual([...diffPairs(diff.lines)], [[1, 2], [2, 1]])
  assert.equal(parseToolDiff('plain output'), null)
})

test('word diff marks only the edited words and gives up on rewrites', () => {
  const words = wordDiff('const [theme] = useState("dark")', 'const [theme, setTheme] = useState("system")')
  assert.ok(words)
  assert.deepEqual(words.before.filter(part => part.changed).map(part => part.text), ['dark'])
  assert.deepEqual(words.after.filter(part => part.changed).map(part => part.text), [', setTheme', 'system'])
  assert.equal(wordDiff('alpha beta gamma', 'totally different words'), null)
})

test('approval previews keep one line of context around the change', () => {
  assert.deepEqual(previewDiff('a\nb\nc', 'a\nB\nc').map(line => `${line.kind[0]}${line.text}`), ['ca', 'db', 'aB', 'cc'])
  assert.deepEqual(previewDiff('', 'new\nfile').map(line => line.kind), ['add', 'add'])
})

async function mount() {
  const client = new FakeBridge()
  const ui = render(React.createElement(App, { client, options: { workspace: 'D:\\demo', provider: 'fake' }, version: '1.0.0' }))
  await delay(80)
  return { client, ui }
}

test('approval panel replaces the prompt and supports arrow selection', async () => {
  const { client, ui } = await mount()
  try {
    ui.stdin.write('run tests')
    ui.stdin.write('\r')
    await delay(40)
    client.emit({
      event: 'approval', sessionId: 'sess-1', approvalId: 'a1',
      request: { tool_name: 'bash', arguments: { command: 'npm test' }, summary: '运行测试' },
    })
    await delay(40)
    const frame = stripAnsi(ui.lastFrame() || '')
    assert.match(frame, /Bash 命令/)
    assert.match(frame, /❯ 1\. 是/)
    assert.doesNotMatch(frame, /输入下一条消息/)
    ui.stdin.write('\x1b[B')
    await delay(30)
    assert.match(stripAnsi(ui.lastFrame() || ''), /❯ 2\. 是，并且本会话不再询问 Bash 命令/)
    ui.stdin.write('\r')
    await delay(30)
    const resolved = client.requests.find(item => item.method === 'resolveApproval')
    assert.deepEqual(resolved?.params, { approvalId: 'a1', granted: true, sessionId: 'sess-1', remember: 'session' })
  } finally {
    ui.unmount()
  }
})

test('shift+tab cycles permission modes and the footer shows the mode', async () => {
  const { client, ui } = await mount()
  try {
    assert.match(stripAnsi(ui.lastFrame() || ''), /\? 查看快捷键/)
    ui.stdin.write('\x1b[Z')
    await delay(40)
    assert.deepEqual(client.requests.filter(item => item.method === 'setPermissionMode').map(item => item.params.mode), ['accept_edits'])
    assert.match(stripAnsi(ui.lastFrame() || ''), /⏵⏵ 自动接受编辑 \(shift\+tab 切换\)/)
    ui.stdin.write('\x1b[Z')
    await delay(40)
    assert.match(stripAnsi(ui.lastFrame() || ''), /\? 查看快捷键/)
  } finally {
    ui.unmount()
  }
})

test('approval letter shortcuts ignore control and alt chords', async () => {
  const { client, ui } = await mount()
  try {
    ui.stdin.write('run tests')
    ui.stdin.write('\r')
    await delay(40)
    client.emit({
      event: 'approval', sessionId: 'sess-1', approvalId: 'a1',
      request: { tool_name: 'bash', arguments: { command: 'npm test' }, summary: '运行测试' },
    })
    await delay(40)
    for (const chord of ['\x01', '\x19', '\x0e', '\x1ba', '\x1by', '\x1bn']) {
      ui.stdin.write(chord)
      await delay(20)
    }
    assert.equal(client.requests.filter(item => item.method === 'resolveApproval').length, 0)
    assert.match(stripAnsi(ui.lastFrame() || ''), /是否继续？/)
    ui.stdin.write('a')
    await delay(30)
    const resolved = client.requests.find(item => item.method === 'resolveApproval')
    assert.deepEqual(resolved?.params, { approvalId: 'a1', granted: true, sessionId: 'sess-1', remember: 'session' })
  } finally {
    ui.unmount()
  }
})

test('? on an empty prompt toggles the shortcut panel without typing', async () => {
  const { ui } = await mount()
  try {
    ui.stdin.write('?')
    await delay(30)
    const open = stripAnsi(ui.lastFrame() || '')
    assert.match(open, /pgup\/pgdn 翻阅历史/)
    assert.match(open, /输入任务/)
    ui.stdin.write('?')
    await delay(30)
    assert.doesNotMatch(stripAnsi(ui.lastFrame() || ''), /pgup\/pgdn 翻阅历史/)
  } finally {
    ui.unmount()
  }
})

for (const columns of [40, 120]) {
  test(`welcome, shortcuts and edit approval fit a ${columns}-column terminal`, async () => {
    const { client, ui } = await mount()
    try {
      Object.defineProperty(ui.stdout, 'columns', { value: columns, writable: true, configurable: true })
      Object.defineProperty(ui.stdout, 'rows', { value: 24, writable: true, configurable: true })
      ui.stdout.emit('resize')
      await delay(40)
      const checkFrame = (expected: RegExp) => {
        const frame = stripAnsi(ui.lastFrame() || '')
        const rows = frame.split('\n')
        assert.equal(rows.length, 24)
        assert.ok(rows.every(row => displayWidth(row) <= columns), frame)
        assert.match(frame, expected)
      }
      checkFrame(/欢迎/)
      ui.stdin.write('?')
      await delay(30)
      checkFrame(/pgup\/pgdn 翻阅历史/)
      ui.stdin.write('?')
      ui.stdin.write('edit file')
      ui.stdin.write('\r')
      await delay(40)
      client.emit({
        event: 'approval', sessionId: 'sess-1', approvalId: 'edit-1',
        request: {
          tool_name: 'edit', summary: 'edit: src/App.tsx',
          arguments: { path: 'src/App.tsx', old_text: 'a\nb\nc\nd\ne\nf\ng', new_text: 'a\nB\nC\nD\nE\nF\ng' },
        },
      })
      await delay(40)
      checkFrame(/3\. 否/)
      ui.stdin.write('\x1b[A')
      await delay(20)
      checkFrame(/❯ 3\. 否/)
      ui.stdin.write('\r')
      await delay(30)
      assert.deepEqual(client.requests.find(item => item.method === 'resolveApproval')?.params,
        { approvalId: 'edit-1', granted: false, sessionId: 'sess-1' })
    } finally {
      ui.unmount()
    }
  })
}
