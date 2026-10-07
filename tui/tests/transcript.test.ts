import assert from 'node:assert/strict'
import test from 'node:test'
import { groupTranscript, lastExpandableTool, layoutTranscript, thoughtLabel, toolOutputLines } from '../src/transcript.js'
import type { FeedItem } from '../src/types.js'

function tool(partial: Partial<FeedItem> & { id: string; name: string }): FeedItem {
  return { kind: 'tool', pending: false, success: true, ...partial }
}

test('quiet reads collapse into a thought line', () => {
  const grouped = groupTranscript([
    { id: 'u1', kind: 'user', text: '看一下' },
    tool({ id: 'c1', name: 'read', args: { path: 'a.py' }, startedAt: 0, endedAt: 900 }),
    tool({ id: 'c2', name: 'grep', args: { pattern: 'foo' }, startedAt: 900, endedAt: 1400 }),
    { id: 'a1', kind: 'assistant', text: '好了' },
  ])
  assert.equal(grouped[1].kind, 'thought')
  if (grouped[1].kind !== 'thought') return
  assert.equal(grouped[1].reads, 1)
  assert.equal(grouped[1].searches, 1)
  assert.match(thoughtLabel(grouped[1]), /读取 1 个文件/)
  assert.match(thoughtLabel(grouped[1]), /搜索 1 次/)
  assert.equal(grouped[2].kind, 'assistant')
})

test('completed assistant stays above later tools', () => {
  const layout = layoutTranscript([
    { id: 'u1', kind: 'user', text: '这个项目是什么' },
    { id: 'a1', kind: 'assistant', text: '先看仓库。' },
    tool({ id: 'b1', name: 'bash', args: { command: 'ls' }, output: 'one\ntwo' }),
  ])
  assert.equal(layout.live.length, 0)
  assert.equal(layout.frozen.map(item => item.kind).join(','), 'user,assistant,tool')
})

test('streaming assistant preserves the whole Markdown document until completion', () => {
  const layout = layoutTranscript([
    { id: 'u1', kind: 'user', text: '问' },
    { id: 's1', kind: 'assistant', pending: true, text: '第一段。\n\n第二段还在写' },
  ])
  assert.equal(layout.frozen.at(-1)?.kind, 'user')
  assert.equal(layout.live.length, 1)
  assert.equal(layout.live[0].text, '第一段。\n\n第二段还在写')
  assert.equal(layout.live[0].pending, true)
})

test('pending tools stay live so order does not invert', () => {
  const layout = layoutTranscript([
    { id: 'u1', kind: 'user', text: '跑' },
    { id: 'a1', kind: 'assistant', text: '开始。' },
    tool({ id: 'b1', name: 'bash', args: { command: 'ls' }, pending: true }),
  ])
  assert.equal(layout.frozen.map(item => item.kind).join(','), 'user,assistant')
  assert.equal(layout.live[0].kind, 'tool')
  assert.equal(layout.live[0].pending, true)
})

test('assistant text is preserved even when its first paragraph repeats the prompt', () => {
  const layout = layoutTranscript([
    { id: 'u1', kind: 'user', text: '这是什么项目' },
    tool({ id: 'b1', name: 'bash', args: { command: 'ls' }, output: 'ok' }),
    { id: 'a1', kind: 'assistant', text: '这是什么项目\n\nMiniCode 是本地编程助手。' },
  ])
  const assistants = [...layout.frozen, ...layout.live].filter(item => item.kind === 'assistant')
  assert.equal(assistants.length, 1)
  assert.equal(assistants[0].text, '这是什么项目\n\nMiniCode 是本地编程助手。')
})

test('bash stays a tree row and is expandable', () => {
  const bash = tool({
    id: 'b1',
    name: 'bash',
    args: { command: 'ls' },
    output: 'one\ntwo\nthree\nfour\nfive\nsix',
  })
  const items = [{ id: 'u1', kind: 'user', text: '跑一下' }, bash]
  const grouped = groupTranscript(items)
  assert.equal(grouped[1].kind, 'tool')
  assert.equal(lastExpandableTool(items, 0)?.id, 'b1')
  assert.equal(toolOutputLines(bash).length, 6)
})
