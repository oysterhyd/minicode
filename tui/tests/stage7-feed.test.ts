import assert from 'node:assert/strict'
import test from 'node:test'
import { applyDesktopEvent, emptyFeed } from '../src/feed.js'

test('TU3 stream keys survive transcript truncation and multiple runs', () => {
  let state = applyDesktopEvent(emptyFeed(), { event: 'text_delta', text: 'first' })
  const first = state.items[0].id
  state = applyDesktopEvent(state, { event: 'run_done', result: { exit_reason: 'completed' } })
  state = applyDesktopEvent({ ...state, items: [] }, { event: 'text_delta', text: 'second' })
  assert.notEqual(state.items[0].id, first)
  const second = state.items[0].id
  state = applyDesktopEvent(state, { event: 'text_delta', text: ' chunk' })
  assert.equal(state.items[0].id, second)
  assert.equal(state.items[0].text, 'second chunk')
})
