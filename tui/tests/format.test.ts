import assert from 'node:assert/strict'
import test from 'node:test'
import { displayWidth, formatElapsed, formatTokens, permissionLabel, truncateWidth, workspaceName } from '../src/format.js'
import { toolMark, toolTarget } from '../src/tools.js'

test('formatTokens matches CLI compact units', () => {
  assert.equal(formatTokens(950), '950')
  assert.equal(formatTokens(12_300), '12.3k')
  assert.equal(formatTokens(300_000), '300k')
  assert.equal(formatTokens(1_048_576), '1M')
})

test('permission and workspace labels', () => {
  assert.equal(permissionLabel('default'), '默认权限')
  assert.equal(permissionLabel('bypass'), '全部允许')
  assert.equal(workspaceName('D:\\\\miniclaudecode\\\\'), 'miniclaudecode')
})

test('tool target and mark', () => {
  assert.equal(toolTarget({ name: 'read', args: { path: 'paginate.py' } }), 'paginate.py')
  assert.equal(toolTarget({ name: 'grep', args: { pattern: 'offset', path: 'src' } }), 'offset  in src')
  assert.equal(toolMark({ id: '1', kind: 'tool', pending: true }), '◌')
  assert.equal(toolMark({ id: '1', kind: 'tool', success: true }), '✓')
  assert.equal(formatElapsed(0, 4800), '4.8s')
})

test('displayWidth counts CJK as two columns', () => {
  assert.equal(displayWidth('ab'), 2)
  assert.equal(displayWidth('中'), 2)
  assert.equal(displayWidth('a中b'), 4)
})

test('truncateWidth keeps CJK width', () => {
  assert.equal(truncateWidth('hello', 10), 'hello')
  assert.equal(truncateWidth('abcdefghij', 6), 'abcde…')
})
