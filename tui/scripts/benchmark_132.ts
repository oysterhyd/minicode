import assert from 'node:assert/strict'
import { writeFileSync } from 'node:fs'
import path from 'node:path'
import { pathToFileURL } from 'node:url'
import { performance } from 'node:perf_hooks'
import type { FeedState } from '../src/feed.js'

const [root, output] = process.argv.slice(2)
const { applyDesktopEvent } = await import(pathToFileURL(path.resolve(root, 'tui/src/feed.ts')).href)
const state: FeedState = {
  items: Array.from({ length: 30000 }, (_, index) => ({ kind: 'notice', id: String(index), text: 'completed' })),
  busy: true, error: '', startedAt: 0, sessionId: null,
}
state.items.push({ kind: 'assistant', id: 'stream', text: 'original', pending: true })
const event = { event: 'text_delta', text: ' appended' }
const samples: number[] = []
for (let repeat = -2; repeat < 7; repeat++) {
  const start = performance.now()
  for (let i = 0; i < 100; i++) {
    const next = applyDesktopEvent(state, event)
    assert.equal(next.items.at(-1)?.text, 'original appended')
    assert.notEqual(next.items, state.items)
    assert.equal(next.items[0], state.items[0])
  }
  if (repeat >= 0) samples.push((performance.now() - start) / 100)
}
assert.equal(state.items.at(-1)?.text, 'original')
const sorted = [...samples].sort((a, b) => a - b)
writeFileSync(output, JSON.stringify({ node: process.version, root: path.resolve(root), samples_ms: samples, median_ms: sorted[3], calls_per_sample: 100 }, null, 2) + '\n')
