import assert from 'node:assert/strict'
import test from 'node:test'
import React from 'react'
import { render } from 'ink-testing-library'
import { App } from '../src/App.js'
import { delay, FakeBridge } from './fake-bridge.js'

test('user bubble and tool line appear in the transcript', async () => {
  const client = new FakeBridge()
  const { stdin, lastFrame, unmount } = render(
    React.createElement(App, { client, options: { workspace: 'D:\\demo', provider: 'fake' }, version: '1.0.0' }),
  )
  try {
    await delay(80)
    stdin.write('修复分页问题')
    await delay(40)
    stdin.write('\r')
    await delay(80)
    client.emit({ event: 'text_delta', sessionId: 'sess-1', text: '我会先核对分页边界。' })
    client.emit({
      event: 'agent_event',
      sessionId: 'sess-1',
      item: {
        seq: 1,
        type: 'tool_call_start',
        timestamp: new Date().toISOString(),
        data: { call_id: 'c1', name: 'read', arguments: { path: 'paginate.py' } },
      },
    })
    client.emit({
      event: 'agent_event',
      sessionId: 'sess-1',
      item: {
        seq: 2,
        type: 'tool_call_result',
        timestamp: new Date().toISOString(),
        data: { call_id: 'c1', success: true, output_preview: 'ok' },
      },
    })
    client.emit({ event: 'run_done', sessionId: 'sess-1', result: { exit_reason: 'completed' } })
    await delay(80)
    const frame = lastFrame() || ''
    assert.match(frame, /修复分页问题/)
    assert.match(frame, /paginate\.py|读取/)
    assert.match(frame, /Thought|读取|Read/)
    assert.match(frame, /MiniCode|MINICODE|minicode/)
    assert.match(frame, /v1\.0\.0/)
    assert.doesNotMatch(frame, /Header|时钟/)
  } finally {
    unmount()
  }
})

test('slash enter opens submenu instead of sending /', async () => {
  const client = new FakeBridge()
  const { stdin, lastFrame, unmount } = render(
    React.createElement(App, { client, options: { workspace: 'D:\\demo', provider: 'fake' }, version: '1.0.0' }),
  )
  try {
    await delay(80)
    stdin.write('/')
    await delay(40)
    stdin.write('\r')
    await delay(80)
    const slash = client.requests.filter(item => item.method === 'runSlash')
    assert.equal(slash.length, 1)
    assert.notEqual(slash[0].params.text, '/')
    assert.match(String(slash[0].params.text), /^\/help$/)
    assert.match(lastFrame() || '', /可用命令|\/help/)
  } finally {
    unmount()
  }
})

test('slash model enter fills submenu then sends selected model', async () => {
  const client = new FakeBridge()
  const { stdin, lastFrame, unmount } = render(
    React.createElement(App, { client, options: { workspace: 'D:\\demo', provider: 'fake' }, version: '1.0.0' }),
  )
  try {
    await delay(80)
    stdin.write('/m')
    await delay(40)
    stdin.write('\r')
    await delay(40)
    assert.equal(client.requests.filter(item => item.method === 'runSlash').length, 0)
    assert.match(lastFrame() || '', /选择模型|fake/)
    stdin.write('\r')
    await delay(80)
    const slash = client.requests.filter(item => item.method === 'runSlash')
    assert.equal(slash.length, 1)
    assert.equal(slash[0].params.text, '/model fake')
  } finally {
    unmount()
  }
})

test('left arrow moves caret so typed text inserts in the middle', async () => {
  const client = new FakeBridge()
  const { stdin, lastFrame, unmount } = render(
    React.createElement(App, { client, options: { workspace: 'D:\\demo', provider: 'fake' }, version: '1.0.0' }),
  )
  try {
    await delay(80)
    stdin.write('hello')
    await delay(20)
    stdin.write('\u001b[D')
    stdin.write('\u001b[D')
    await delay(20)
    stdin.write('X')
    await delay(40)
    assert.match(lastFrame() || '', /helXlo/)
  } finally {
    unmount()
  }
})

test('windows backspace (DEL) deletes backward', async () => {
  const client = new FakeBridge()
  const { stdin, lastFrame, unmount } = render(
    React.createElement(App, { client, options: { workspace: 'D:\\demo', provider: 'fake' }, version: '1.0.0' }),
  )
  try {
    await delay(80)
    stdin.write('hello')
    await delay(20)
    stdin.write('\x7f')
    await delay(40)
    const frame = lastFrame() || ''
    assert.match(frame, /hell/)
    assert.doesNotMatch(frame, /hello/)
  } finally {
    unmount()
  }
})

test('ctrl+o expands collapsed bash output', async () => {
  const client = new FakeBridge()
  const { stdin, lastFrame, unmount } = render(
    React.createElement(App, { client, options: { workspace: 'D:\\demo', provider: 'fake' }, version: '1.0.0' }),
  )
  try {
    await delay(80)
    stdin.write('跑命令')
    await delay(20)
    stdin.write('\r')
    await delay(40)
    client.emit({
      event: 'agent_event',
      sessionId: 'sess-1',
      item: {
        seq: 1,
        type: 'tool_call_start',
        timestamp: new Date().toISOString(),
        data: { call_id: 'bash-1', name: 'bash', arguments: { command: 'seq 6' } },
      },
    })
    client.emit({
      event: 'agent_event',
      sessionId: 'sess-1',
      item: {
        seq: 2,
        type: 'tool_call_result',
        timestamp: new Date().toISOString(),
        data: { call_id: 'bash-1', success: true, output_preview: '1\n2\n3\n4\n5\n6' },
      },
    })
    await delay(40)
    const collapsed = lastFrame() || ''
    assert.match(collapsed, /Bash|seq 6/)
    assert.doesNotMatch(collapsed, /⎿/)
    stdin.write('\x0f')
    await delay(40)
    const opened = lastFrame() || ''
    assert.match(opened, /⎿/)
    assert.match(opened, /6/)
  } finally {
    unmount()
  }
})

test('approval block is answered with y', async () => {
  const client = new FakeBridge()
  const { stdin, lastFrame, unmount } = render(
    React.createElement(App, { client, options: { workspace: 'D:\\demo', provider: 'fake' }, version: '1.0.0' }),
  )
  try {
    await delay(80)
    client.emit({
      event: 'approval',
      sessionId: 'sess-1',
      approvalId: 'a1',
      request: { tool_name: 'write', arguments: { path: 'new.txt', content: 'hello' }, summary: 'write new.txt' },
    })
    await delay(40)
    assert.match(lastFrame() || '', /需要你的批准/)
    stdin.write('y')
    await delay(40)
    const resolved = client.requests.find(item => item.method === 'resolveApproval')
    assert.ok(resolved)
    assert.equal(resolved.params.granted, true)
  } finally {
    unmount()
  }
})
