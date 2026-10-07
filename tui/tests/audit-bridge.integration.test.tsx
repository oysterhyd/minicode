import assert from 'node:assert/strict'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { setTimeout as delay } from 'node:timers/promises'
import { test } from 'node:test'
import React from 'react'
import { render } from 'ink-testing-library'
import { App } from '../src/App.js'
import { BridgeClient } from '../src/bridge.js'
import { spawnBridge } from '../src/launch.js'
import { historyItems, type AgentState, type DesktopEvent, type SessionDetail } from '../src/types.js'

const projectRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..')
const enabled = process.env.MINICODE_TUI_BRIDGE_TEST === '1'

async function waitFor(check: () => boolean, description: string) {
  for (let attempt = 0; attempt < 600; attempt++) {
    if (check()) return
    await delay(10)
  }
  assert.ok(check(), description)
}

function fixture(turns: unknown[]) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'minicode-tui-audit-'))
  assert.equal(path.dirname(root), path.resolve(os.tmpdir()))
  const workspace = path.join(root, 'workspace')
  fs.mkdirSync(workspace)
  fs.writeFileSync(path.join(workspace, 'README.md'), 'Independent TUI audit fixture.\n')
  const script = path.join(root, 'script.json')
  fs.writeFileSync(script, JSON.stringify({ turns }))
  const child = spawnBridge({ projectRoot, extraEnv: {
    MINICODE_DB_PATH: path.join(root, 'sessions.db'), MINICODE_FAKE_SCRIPT: script,
    USERPROFILE: root, HOME: root,
  } })
  const client = new BridgeClient(child)
  const events: DesktopEvent[] = []
  client.onEvent(event => events.push(event))
  return { root, workspace, child, client, events, async close() {
    await client.close()
    assert.equal(path.dirname(root), path.resolve(os.tmpdir()))
    fs.rmSync(root, { recursive: true, force: true })
  } }
}

test('real Ink + Python: script selection, approvals, pause/continue, persisted history and graceful exit', { skip: !enabled, timeout: 30000 }, async t => {
  const f = fixture([
    { text: '先读取项目，再完成修改。', tool_calls: [
      { name: 'read', arguments: { path: 'README.md' } },
      { name: 'write', arguments: { path: 'note.txt', content: 'approved write' } },
    ] },
    { text: '修改已完成。\n\n- 支持中文与 **Markdown**。' },
  ])
  t.after(() => f.close())
  const ui = render(<App client={f.client} options={{ workspace: f.workspace, script: 'script.json', maxRounds: 1 }} />)
  t.after(() => { ui.unmount(); ui.cleanup() })
  await waitFor(() => (ui.lastFrame() || '').includes('fake') && !(ui.lastFrame() || '').includes('正在连接'), 'TUI ready with an implicit fake script')
  ui.stdin.write('检查这个项目')
  ui.stdin.write('\r')
  await waitFor(() => f.events.some(event => event.event === 'approval'), 'write waits for user approval')
  assert.equal(fs.existsSync(path.join(f.workspace, 'note.txt')), false)
  ui.stdin.write('y')
  await waitFor(() => f.events.some(event => event.event === 'run_done'), 'round slice finishes')
  assert.equal(f.events.find(event => event.event === 'run_done')?.result?.exit_reason, 'max_rounds')
  assert.equal(fs.readFileSync(path.join(f.workspace, 'note.txt'), 'utf8'), 'approved write')
  const sessionId = f.events.find(event => event.sessionId)?.sessionId
  assert.ok(sessionId)
  let state = await f.client.request<AgentState>('getState', { sessionId })
  assert.equal(state.taskPending, true)
  ui.stdin.write('/continue')
  ui.stdin.write('\r')
  await waitFor(() => f.events.filter(event => event.event === 'run_done').length === 2, 'continue finishes')
  assert.equal(f.events.filter(event => event.event === 'run_done')[1].result?.exit_reason, 'completed')
  const detail = await f.client.request<SessionDetail>('getSession', { sessionId })
  assert.equal(fs.realpathSync(detail.summary.workspace), fs.realpathSync(f.workspace))
  const history = historyItems(detail)
  assert.ok(history.some(item => item.kind === 'tool' && item.name === 'read' && item.success && item.output?.includes('Independent TUI audit')))
  assert.ok(history.some(item => item.kind === 'assistant' && item.text?.includes('修改已完成')))
  assert.doesNotMatch(ui.lastFrame() || '', /Agent bridge.*出错/)
  await f.client.request('runSlash', { text: '/new', workspace: f.workspace, sessionId })
  state = await f.client.request<AgentState>('getState')
  assert.equal(state.sessionId, null)
  assert.equal(state.rounds, 0)
  assert.equal(state.contextTokens, 0)
  assert.equal(state.taskPending, false)
  await f.client.close()
  assert.equal(f.child.exitCode, 0)
})

test('real Python: cancellation while waiting for approval does not write the file and permits recovery', { skip: !enabled, timeout: 30000 }, async t => {
  const f = fixture([{ tool_calls: [{ name: 'write', arguments: { path: 'cancelled.txt', content: 'must not be written' } }] }])
  t.after(() => f.close())
  await f.client.request('initialize', { workspace: f.workspace })
  await f.client.request('setTuiProvider', { provider: 'fake' })
  const started = await f.client.request<{ sessionId: string }>('sendPrompt', { workspace: f.workspace, text: 'wait for approval', model: 'fake' })
  await waitFor(() => f.events.some(event => event.event === 'approval'), 'approval is pending')
  await f.client.request('cancelTurn', { sessionId: started.sessionId })
  await waitFor(() => f.events.some(event => event.event === 'run_done'), 'cancellation completes')
  assert.equal(f.events.find(event => event.event === 'run_done')?.result?.exit_reason, 'cancelled')
  assert.equal(fs.existsSync(path.join(f.workspace, 'cancelled.txt')), false)
  const state = await f.client.request<AgentState>('getState', { sessionId: started.sessionId })
  assert.equal(state.running, false)
  assert.equal(state.taskPending, true)
  const approvals = f.events.filter(event => event.event === 'approval')
  assert.equal(await f.client.request('resolveApproval', { sessionId: started.sessionId, approvalId: approvals[0].approvalId, granted: true }), false)
})

test('real Python: explicit providers cannot silently switch to another provider', { skip: !enabled, timeout: 30000 }, async t => {
  const f = fixture([{ text: 'offline' }])
  t.after(() => f.close())
  await f.client.request('initialize', { workspace: f.workspace })
  await assert.rejects(f.client.request('setTuiProvider', { provider: 'anthropic', model: 'deepseek/deepseek-v4.1-flash' }), /不属于 provider/)
  const state = await f.client.request<AgentState>('setTuiProvider', { provider: 'auto' })
  assert.equal(state.model, 'fake')
})

test('real Python: TUI scripts reject misspelled option, turn and tool-call fields', { skip: !enabled, timeout: 30000 }, async t => {
  const f = fixture([])
  t.after(() => f.close())
  await f.client.request('initialize', { workspace: f.workspace })
  for (const script of [
    { truns: [] },
    { turns: [{ txet: 'misspelled' }] },
    { turns: [{ tool_calls: [{ name: 'write', argumnts: {} }] }] },
  ]) {
    fs.writeFileSync(path.join(f.root, 'script.json'), JSON.stringify(script))
    await assert.rejects(f.client.request('setTuiProvider', { provider: 'fake' }), /包含未知字段/)
  }
})
