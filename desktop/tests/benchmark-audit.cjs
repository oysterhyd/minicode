'use strict'
// node --expose-gc desktop/tests/benchmark-audit.cjs [source-root] [output.json]
// Loads actual TypeScript in memory. No DOM, network, or production profile.
const fs = require('node:fs')
const path = require('node:path')
const vm = require('node:vm')
const { performance } = require('node:perf_hooks')
const ts = require('typescript')
const root = path.resolve(process.argv[2] || path.join(__dirname, '../..'))
function load(file, source = fs.readFileSync(file, 'utf8')) {
  const code = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.CommonJS } }).outputText
  const api = { exports: {} }
  vm.runInNewContext(code, api, { filename: file })
  return api.exports
}
function measure(fn, repeats = 7) {
  fn(); fn()
  const samples = []
  for (let i = 0; i < repeats; i++) {
    global.gc?.()
    const start = performance.now()
    fn()
    samples.push(performance.now() - start)
  }
  samples.sort((a, b) => a - b)
  return { median_ms: +samples[Math.floor(repeats / 2)].toFixed(4), min_ms: +samples[0].toFixed(4), max_ms: +samples.at(-1).toFixed(4), repeats }
}
const diff = load(path.join(root, 'desktop/src/lib/diff.ts'))
const feedPath = path.join(root, 'desktop/src/lib/feed.ts')
let group
if (fs.existsSync(feedPath)) group = load(feedPath).groupFeedItems
else {
  const original = fs.readFileSync(path.join(root, 'desktop/src/components/feed/SessionFeed.tsx'), 'utf8')
  const body = original.slice(original.indexOf('type Row'), original.indexOf('export function SessionFeed'))
  group = load(feedPath, body + '\nexport { rows as groupFeedItems }').groupFeedItems
}
const items = Array.from({ length: 30_000 }, (_, i) => ({ id: String(i), kind: i % 2 ? 'assistant' : 'user' }))
const before = Array.from({ length: 400 }, (_, i) => `line-${i}`)
const after = [...before]
after[100] = 'edit-100'; after[300] = 'edit-300'
const largeBefore = Array.from({ length: 10_000 }, (_, i) => `line-${i}`)
const largeAfter = [...largeBefore]
for (const i of [100, 5000, 9900]) largeAfter[i] = `edit-${i}`
const beforeText = before.join('\n'), afterText = after.join('\n')
const largeA = largeBefore.join('\n'), largeB = largeAfter.join('\n')
const largeResult = diff.diffTexts(largeA, largeB)
const result = { source_root: root, node: process.version, platform: process.platform, benchmarks: {
  feed_group_and_positions_30k_rows: measure(() => {
    let sum = 0
    for (const row of group(items)) if (row.type === 'item') sum += row.position ?? items.indexOf(row.item)
    if (sum !== 30_000 * 29_999 / 2) throw new Error('wrong source positions')
  }),
  diff_400_lines_two_edits: measure(() => diff.diffTexts(beforeText, afterText)),
  diff_10k_lines_three_edits: { ...measure(() => diff.diffTexts(largeA, largeB)), output_rows: largeResult.length, ...diff.diffStats(largeResult) },
} }
const output = JSON.stringify(result, null, 2)
if (process.argv[3]) fs.writeFileSync(process.argv[3], output + '\n')
console.log(output)
