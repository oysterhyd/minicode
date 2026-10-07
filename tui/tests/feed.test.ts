import assert from 'node:assert/strict'
import test from 'node:test'
import { applyDesktopEvent, emptyFeed, isLive, pendingApproval } from '../src/feed.js'

test('text delta then assistant message becomes one reply', () => {
  let state = emptyFeed()
  state = applyDesktopEvent(state, { event: 'text_delta', text: '我会先' })
  state = applyDesktopEvent(state, { event: 'text_delta', text: '核对分页。' })
  state = applyDesktopEvent(state, {
    event: 'agent_event',
    item: { seq: 1, type: 'assistant_message', timestamp: '2026-01-01T00:00:00Z', data: { text: '我会先核对分页。' } },
  })
  assert.equal(state.items.length, 1)
  assert.equal(state.items[0].kind, 'assistant')
  assert.equal(state.items[0].text, '我会先核对分页。')
  assert.equal(state.items[0].pending, undefined)
})

test('tool start and result stay one line', () => {
  let state = emptyFeed()
  state = applyDesktopEvent(state, {
    event: 'agent_event',
    item: {
      seq: 2,
      type: 'tool_call_start',
      timestamp: '2026-01-01T00:00:00Z',
      data: { call_id: 'c1', name: 'read', arguments: { path: 'paginate.py' } },
    },
  })
  assert.equal(state.items[0].pending, true)
  state = applyDesktopEvent(state, {
    event: 'agent_event',
    item: {
      seq: 3,
      type: 'tool_call_result',
      timestamp: '2026-01-01T00:00:01Z',
      data: { call_id: 'c1', success: true, output_preview: 'ok' },
    },
  })
  assert.equal(state.items[0].pending, false)
  assert.equal(state.items[0].success, true)
  assert.equal(state.items[0].output, 'ok')
})

test('approval sits in the feed until decided', () => {
  let state = emptyFeed()
  state = applyDesktopEvent(state, {
    event: 'approval',
    approvalId: 'a1',
    request: { tool_name: 'write', arguments: { path: 'new.txt' }, summary: 'write new.txt' },
  })
  assert.equal(pendingApproval(state.items)?.approvalId, 'a1')
  state = applyDesktopEvent(state, {
    event: 'agent_event',
    item: { seq: 4, type: 'approval_decision', timestamp: '2026-01-01T00:00:02Z', data: { granted: true } },
  })
  assert.equal(pendingApproval(state.items), undefined)
  assert.equal(state.items[0].granted, true)
  assert.equal(isLive(state.items[0]), false)
})

test('streaming assistant and pending tools stay live', () => {
  let state = emptyFeed()
  state = applyDesktopEvent(state, { event: 'text_delta', text: '流式' })
  assert.equal(isLive(state.items[0]), true)
  state = applyDesktopEvent(state, {
    event: 'agent_event',
    item: {
      seq: 5,
      type: 'tool_call_start',
      timestamp: '2026-01-01T00:00:03Z',
      data: { call_id: 'c2', name: 'read', arguments: { path: 'a.py' } },
    },
  })
  assert.equal(isLive(state.items.at(-1)!), true)
})
