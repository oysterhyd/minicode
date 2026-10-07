'use strict'
const { test } = require('node:test')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const path = require('node:path')
const vm = require('node:vm')
const ts = require('typescript')
const file = path.join(__dirname, '../src/lib/feed.ts')
const code = ts.transpileModule(fs.readFileSync(file, 'utf8'), { compilerOptions: { module: ts.ModuleKind.CommonJS } }).outputText
const api = { exports: {} }
vm.runInNewContext(code, api, { filename: file })
const { groupFeedItems } = api.exports

test('grouping retains original positions, consecutive tools and item identity', () => {
  const items = [
    { id: '1', kind: 'user' }, { id: '2', kind: 'tool' }, { id: '3', kind: 'tool' },
    { id: '4', kind: 'assistant' }, { id: '5', kind: 'tool' }, { id: '6', kind: 'notice' },
  ]
  const grouped = groupFeedItems(items)
  assert.equal(grouped.length, 5)
  assert.equal(grouped[0].position, 0)
  assert.equal(grouped[2].position, 3)
  assert.equal(grouped[4].position, 5)
  assert.equal(grouped[2].item, items[3])
  assert.equal(grouped[1].items.length, 2)
  assert.equal(grouped[1].items[0], items[1])
  assert.equal(items.length, 6)
})

test('30k rows never search the source history for a row position', () => {
  const items = Array.from({ length: 30_000 }, (_, i) => ({ id: String(i), kind: i % 2 ? 'assistant' : 'user' }))
  items.indexOf = () => { throw new Error('quadratic history lookup') }
  const grouped = groupFeedItems(items)
  for (let i = 0; i < grouped.length; i++) assert.equal(grouped[i].position, i)
})
