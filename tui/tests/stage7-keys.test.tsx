import assert from 'node:assert/strict'
import test from 'node:test'
import React from 'react'
import { render } from 'ink-testing-library'
import { App } from '../src/App.js'
import { delay, FakeBridge } from './fake-bridge.js'

test('TU7 Esc cancels a run once and double Esc copies the last user message to the draft', async () => {
  const client = new FakeBridge()
  const view = render(<App client={client} options={{ workspace: '/repo', provider: 'fake' }} />)
  try {
    await delay(100); view.stdin.write('last user message'); view.stdin.write('\r'); await delay(40)
    view.stdin.write('\x1b'); await delay(20); view.stdin.write('\x1b'); await delay(20)
    assert.equal(client.requests.filter(r => r.method === 'cancelTurn').length, 1)
    client.emit({ event: 'run_done', sessionId: 'sess-1', result: { exit_reason: 'cancelled' } })
    await delay(450)
    view.stdin.write('\x1b'); await delay(20); view.stdin.write('\x1b'); await delay(40)
    assert.match(view.lastFrame() || '', /last user message/)
    assert.equal(client.requests.filter(r => r.method === 'sendPrompt').length, 1)
  } finally { view.unmount() }
})

test('T-UI3 capable bridge owns busy prompts and distinguishes follow-up', async () => {
  const client = new FakeBridge()
  const initial = await client.request<any>('initialize')
  client.handlers.set('initialize', () => ({ ...initial, capabilities: ['messageQueue'] }))
  client.handlers.set('enqueuePrompt', p => ({ id: 'queued', ...p }))
  client.handlers.set('listQueue', () => [])
  const view = render(<App client={client} options={{ workspace: '/repo', provider: 'fake' }} />)
  try {
    await delay(100); view.stdin.write('first'); view.stdin.write('\r'); await delay(30)
    view.stdin.write('steer now'); view.stdin.write('\r'); await delay(30)
    view.stdin.write('/follow later'); view.stdin.write('\r'); await delay(40)
    assert.deepEqual(client.requests.filter(r => r.method === 'enqueuePrompt').map(r => r.params.mode), ['steer', 'followUp'])
    client.emit({ event: 'run_done', sessionId: 'sess-1', result: { exit_reason: 'completed' } })
    await delay(30)
    assert.equal(client.requests.filter(r => r.method === 'sendPrompt').length, 1)
  } finally { view.unmount() }
})
test('T-UI3 queue deletion failure is recoverable and malformed commands are rejected', async () => {
  const client = new FakeBridge(), initial = await client.request<any>('initialize')
  client.handlers.set('initialize', () => ({ ...initial, capabilities: ['messageQueue'] }))
  client.handlers.set('listQueue', () => [])
  client.handlers.set('deleteQueuedPrompt', () => { throw Error('cannot delete queued message') })
  const view = render(<App client={client} options={{ workspace: '/repo', provider: 'fake' }} />)
  try {
    await delay(100); view.stdin.write('first'); view.stdin.write('\r'); await delay(30)
    view.stdin.write('/queue cancel id'); view.stdin.write('\r'); await delay(50)
    assert.match(view.lastFrame() || '', /cannot delete queued message/)
    view.stdin.write('/queue cancel'); view.stdin.write('\r'); await delay(30)
    assert.match(view.lastFrame() || '', /用法/)
  } finally { view.unmount() }
})
