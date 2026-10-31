import assert from 'node:assert/strict'
import test from 'node:test'
import React from 'react'
import { render } from 'ink-testing-library'
import { MarkdownView, table } from '../src/markdown.js'
import type { Tokens } from 'marked'
import stripAnsi from 'strip-ansi'

test('markdown renders headings, bold, code, tables and math', () => {
  const source = [
    '# 标题一',
    '',
    '这是 **加粗** 和 `code`。',
    '',
    '```ts',
    'const n = 1',
    '```',
    '',
    '| a | b |',
    '| --- | --- |',
    '| 1 | 2 |',
    '',
    '$$E=mc^2$$',
  ].join('\n')
  const { lastFrame, unmount } = render(React.createElement(MarkdownView, { text: source }))
  try {
    const frame = lastFrame() || ''
    assert.match(frame, /标题一/)
    assert.match(frame, /加粗/)
    assert.match(frame, /code/)
    assert.match(frame, /const n = 1/)
    assert.match(frame, /ts/)
    assert.match(frame, /1/)
    assert.match(frame, /2/)
    assert.match(frame, /E=mc\^2/)
  } finally {
    unmount()
  }
})

test('TU4 narrow tables omit an empty header prefix', () => {
  const cell = (text: string) => ({ text, tokens: [{ type: 'text', text }] })
  const view = render(table({ header: [cell('Very long heading')], rows: [[cell('one'), cell('value')]], align: [] } as unknown as Tokens.Table, 10))
  try {
    assert.match(view.lastFrame() || '', /value/)
    assert.doesNotMatch(stripAnsi(view.lastFrame() || ''), /: value/)
  } finally { view.unmount() }
})
