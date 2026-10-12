import assert from 'node:assert/strict'
import test from 'node:test'
import { lastExpandableTool, layoutTranscript } from '../src/transcript.js'
import type { FeedItem } from '../src/types.js'
import { applyDesktopEvent, emptyFeed } from '../src/feed.js'

test('expandable tool search respects the current user boundary', () => {
  const items: FeedItem[] = [
    { id: 'old', kind: 'tool', output: 'old output' },
    { id: 'user', kind: 'user', text: 'next turn' },
    { id: 'empty', kind: 'tool', name: 'read' },
    { id: 'command', kind: 'tool', args: { command: 'pwd' } },
  ]
  assert.equal(lastExpandableTool(items, 1), items[3])
  assert.equal(lastExpandableTool(items, 4), undefined)
  assert.equal(lastExpandableTool(items.slice(0, 3), 1), undefined)
  assert.equal(lastExpandableTool(items.slice(0, 3), -1), items[0])
  assert.equal(lastExpandableTool([], 0), undefined)
})

test('finishing a run preserves settled rows and only replaces unfinished rows', () => {
  const settled: FeedItem = { id: 'old', kind: 'assistant', text: 'unchanged' }
  const pending: FeedItem = { id: 'live', kind: 'tool', name: 'bash', pending: true }
  const approval: FeedItem = { id: 'approve', kind: 'approval', approvalId: 'a' }
  const state = { ...emptyFeed(), items: [settled, pending, approval], busy: true }
  const done = applyDesktopEvent(state, { event: 'run_done', result: { exit_reason: 'completed' } })
  assert.equal(done.items[0], settled)
  assert.equal(done.items[1].pending, false)
  assert.equal(done.items[1].interrupted, true)
  assert.equal(done.items[2].approvalId, undefined)
  assert.equal(pending.pending, true)
  assert.equal(approval.approvalId, 'a')
  const unmatched = applyDesktopEvent(state, { event: 'agent_event', item: { type: 'tool_output', seq: 1, timestamp: '', data: { call_id: 'missing' } } })
  assert.equal(unmatched.items, state.items)
})

test('large quiet groups do not depend on JavaScript argument limits', () => {
  const items: FeedItem[] = Array.from({ length: 150000 }, (_, i) => (
    { id: String(i), kind: 'tool', name: 'read', success: true, endedAt: i + 1 }
  ))
  const complete = layoutTranscript(items)
  assert.equal(complete.frozen.length, 1)
  assert.equal(complete.frozen[0].kind, 'thought')
  assert.equal(complete.frozen[0].endedAt, 150000)
  items.at(-1)!.pending = true
  const pending = layoutTranscript(items)
  assert.equal(pending.frozen.length, 0)
  assert.equal(pending.live.length, items.length)
  assert.equal(pending.live.at(-1), items.at(-1))
})
