import assert from 'node:assert/strict'
import test from 'node:test'
import { InputHistory, PasteStore, fileMention } from '../src/input-history.js'

test('T-UI6 history restores the draft and ignores duplicate empty entries', () => {
  const history = new InputHistory()
  history.add('one'); history.add('two'); history.add('two'); history.add('')
  assert.equal(history.previous('draft'), 'two')
  assert.equal(history.previous('two'), 'one')
  assert.equal(history.next(), 'two')
  assert.equal(history.next(), 'draft')
  assert.equal(history.next('ordinary draft'), 'ordinary draft')
})
test('T-UI6 10000-line paste stays compact, expands losslessly and cancels', () => {
  const store = new PasteStore()
  const text = '一行 😀\n'.repeat(10000)
  const draft = store.fold(text)
  assert.ok(draft.length < 80)
  assert.equal(store.expand(`before ${draft} after`), `before ${text} after`)
  assert.equal(store.preview(1).length, 4000)
  store.cancel(1)
  assert.throws(() => store.expand(draft), /粘贴/)
  assert.deepEqual(fileMention('read @"目录/file name"'), { start: 5, query: '目录/file name' })
})
