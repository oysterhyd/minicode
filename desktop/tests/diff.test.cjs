'use strict'
const assert = require('node:assert/strict')
const fs = require('node:fs')
const path = require('node:path')
const vm = require('node:vm')
const { test } = require('node:test')
const ts = require('typescript')

const filename = path.join(__dirname, '../src/lib/diff.ts')
const source = fs.readFileSync(filename, 'utf8')
const code = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.CommonJS } }).outputText
const api = { exports: {} }
vm.runInNewContext(code, api, { filename })
const { parseUnifiedDiff, diffTexts, diffStats } = api.exports
const plain = value => JSON.parse(JSON.stringify(value))

test('unified hunk content resembling file headers remains additions/removals', () => {
  const result = plain(parseUnifiedDiff('--- a/file\n+++ b/file\n@@ -4,2 +4,2 @@\n--- old header\n+++ new header\n context\n'))
  assert.deepEqual(result.slice(3), [
    { type: 'remove', text: '-- old header', oldLine: 4 },
    { type: 'add', text: '++ new header', newLine: 4 },
    { type: 'context', text: 'context', oldLine: 5, newLine: 5 },
  ])
})

test('unified hunks do not invent a context line at the terminating newline', () => {
  const result = plain(parseUnifiedDiff('@@ -0,0 +1 @@\n+hello\n\\ No newline at end of file\n'))
  assert.deepEqual(result, [
    { type: 'hunk', text: '@@ -0,0 +1 @@' },
    { type: 'add', text: 'hello', newLine: 1 },
    { type: 'meta', text: '\\ No newline at end of file' },
  ])
})

test('unified parser handles empty content lines and multiple files', () => {
  const result = plain(parseUnifiedDiff('diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1,2 @@\n same\n+\ndiff --git a/b b/b\n--- a/b\n+++ b/b\n@@ -1 +1 @@\n-old\n+new\n'))
  assert.deepEqual(result.filter(line => ['add', 'remove', 'context'].includes(line.type)), [
    { type: 'context', text: 'same', oldLine: 1, newLine: 1 },
    { type: 'add', text: '', newLine: 2 },
    { type: 'remove', text: 'old', oldLine: 1 },
    { type: 'add', text: 'new', newLine: 1 },
  ])
})

test('an empty snippet is not a phantom removed/added line', () => {
  assert.deepEqual(plain(diffTexts('', '')), [])
  assert.deepEqual(plain(diffTexts('', 'hello')), [{ type: 'add', text: 'hello', newLine: 1 }])
  assert.deepEqual(plain(diffTexts('hello', '')), [{ type: 'remove', text: 'hello', oldLine: 1 }])
})

test('large similar files retain unchanged context instead of replacing the entire file', () => {
  const a = Array.from({ length: 10_000 }, (_, i) => `line-${i}`)
  const b = [...a]
  for (const i of [100, 5000, 9900]) b[i] = `changed-${i}`
  const lines = plain(diffTexts(a.join('\n'), b.join('\n')))
  assert.deepEqual(plain(diffStats(lines)), { additions: 3, deletions: 3 })
  assert.equal(lines.length, 10_003)
  assert.deepEqual(lines.filter(line => line.type !== 'add').map(line => line.text), a)
  assert.deepEqual(lines.filter(line => line.type !== 'remove').map(line => line.text), b)
})

test('pathological disjoint files use the bounded, correct remove/add fallback', () => {
  const a = Array.from({ length: 1000 }, (_, i) => `old-${i}`)
  const b = Array.from({ length: 1000 }, (_, i) => `new-${i}`)
  const lines = plain(diffTexts(a.join('\n'), b.join('\n')))
  assert.deepEqual(plain(diffStats(lines)), { additions: 1000, deletions: 1000 })
  assert.deepEqual(lines.filter(line => line.type !== 'add').map(line => line.text), a)
  assert.deepEqual(lines.filter(line => line.type !== 'remove').map(line => line.text), b)
})

function lcs(a, b) {
  const rows = Array.from({ length: a.length + 1 }, () => Array(b.length + 1).fill(0))
  for (let i = a.length - 1; i >= 0; i--) for (let j = b.length - 1; j >= 0; j--) {
    rows[i][j] = a[i] === b[j] ? rows[i + 1][j + 1] + 1 : Math.max(rows[i + 1][j], rows[i][j + 1])
  }
  return rows[0][0]
}

test('snippet diff reconstructs both inputs and has minimal edit count', () => {
  let seed = 20260930
  const random = () => { seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0; return seed }
  for (let run = 0; run < 300; run++) {
    const a = Array.from({ length: random() % 16 + 1 }, () => String(random() % 7))
    const b = Array.from({ length: random() % 16 + 1 }, () => String(random() % 7))
    const lines = plain(diffTexts(a.join('\n'), b.join('\n')))
    assert.deepEqual(lines.filter(line => line.type !== 'add').map(line => line.text), a)
    assert.deepEqual(lines.filter(line => line.type !== 'remove').map(line => line.text), b)
    const stats = diffStats(lines)
    assert.equal(stats.additions + stats.deletions, a.length + b.length - 2 * lcs(a, b))
    assert.deepEqual(lines.filter(line => line.oldLine).map(line => line.oldLine), a.map((_, i) => i + 1))
    assert.deepEqual(lines.filter(line => line.newLine).map(line => line.newLine), b.map((_, i) => i + 1))
  }
})
