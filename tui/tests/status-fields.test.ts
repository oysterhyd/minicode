import assert from 'node:assert/strict'
import test from 'node:test'
import { statusFields } from '../src/components/StatusLine.js'
import { emptyState } from '../src/types.js'
test('TU8 configurable status fields safely represent missing data', () => {
  const text = statusFields({ ...emptyState, model: 'fake', rounds: 3 }, ['model', 'turns', 'budget', 'cost', 'branch', 'elapsed'], 80, 4200)
  assert.match(text, /fake/); assert.match(text, /3/); assert.match(text, /不可用/)
  assert.ok(!text.includes('undefined'))
  assert.equal(statusFields(emptyState, ['unknown'], 10, 0), '')
})
test('TU8 budget includes round and time limits when tokens are unlimited', () => {
  const text = statusFields({ ...emptyState, budget: { max_rounds: 8, max_total_tokens: 0, max_seconds: 300 } }, ['budget'], 80, 0)
  assert.match(text, /8/); assert.match(text, /300/); assert.doesNotMatch(text, /不限/)
  assert.match(statusFields({ ...emptyState, budget: { max_rounds: 0, max_total_tokens: 1000, max_seconds: 0 } }, ['budget'], 80, 0), /tokens/)
})
