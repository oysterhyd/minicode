import assert from 'node:assert/strict'
import test from 'node:test'
import {
  caretCoords,
  deleteBackward,
  insertAt,
  moveVertical,
  nextBoundary,
  prevBoundary,
  takeChar,
} from '../src/caret.js'

test('boundaries skip surrogate pairs', () => {
  const text = 'a😀b'
  assert.equal(nextBoundary(text, 1), 3)
  assert.equal(prevBoundary(text, 3), 1)
})

test('insert and delete keep caret in the middle', () => {
  assert.equal(insertAt('hello', 2, 'X'), 'heXllo')
  assert.deepEqual(deleteBackward('hello', 2), { text: 'hllo', caret: 1 })
})

test('vertical move preserves column', () => {
  const text = 'abc\nde'
  const start = caretCoords(text, 2)
  assert.equal(start.line, 0)
  assert.equal(start.column, 2)
  assert.equal(moveVertical(text, 2, 1), 6)
})

test('takeChar splits one grapheme-ish unit', () => {
  assert.deepEqual(takeChar('中文'), ['中', '文'])
  assert.deepEqual(takeChar(''), ['', ''])
})
