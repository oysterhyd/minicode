import assert from 'node:assert/strict'
import test from 'node:test'
import { lastExpandableTool, layoutTranscript } from '../src/transcript.js'
import type { FeedItem } from '../src/types.js'

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
