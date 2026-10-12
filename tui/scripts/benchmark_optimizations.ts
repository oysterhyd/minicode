import { performance } from 'node:perf_hooks'
import { writeFileSync } from 'node:fs'
import { lastExpandableTool } from '../src/transcript.js'
import type { FeedItem } from '../src/types.js'

// Exact previous implementation, retained only as a differential benchmark.
function reference(items: FeedItem[], afterUserIndex: number) {
  return [...items].reverse().find(item => (
    item.kind === 'tool' && items.indexOf(item) >= afterUserIndex
    && Boolean(item.output || item.args?.command || item.name?.startsWith('skill'))
  ))
}
const items: FeedItem[] = Array.from({ length: 10000 }, (_, i) => (
  { id: String(i), kind: 'tool', name: 'read' }
))
function measure(fn: () => unknown) {
  fn()
  const values: number[] = []
  for (let i = 0; i < 7; i++) {
    const start = performance.now()
    for (let n = 0; n < 10; n++) fn()
    values.push((performance.now() - start) / 10)
  }
  values.sort((a, b) => a - b)
  return { median_ms: values[3], min_ms: values[0], max_ms: values[6], repeats: 7 }
}
if (reference(items, 0) !== lastExpandableTool(items, 0)) throw new Error('Behavior changed')
const before = measure(() => reference(items, 0))
const after = measure(() => lastExpandableTool(items, 0))
const output = process.argv[2]
if (!output) throw new Error('Pass an output JSON path')
writeFileSync(output, JSON.stringify({ node: process.version, benchmark: '10000 quiet tools without expandable output', before, after }, null, 2) + '\n')
