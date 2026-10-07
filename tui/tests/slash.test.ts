import assert from 'node:assert/strict'
import test from 'node:test'
import { applySuggestion, isSubmenu, slashSuggestions } from '../src/slash.js'

const context = {
  commands: [
    { name: '/help', usage: '/help', summary: '显示本帮助' },
    { name: '/model', usage: '/model [名称]', summary: '查看或切换活跃模型' },
    { name: '/permissions', usage: '/permissions [default|accept_edits|bypass]', summary: '权限' },
  ],
  models: [{ id: 'fake', available: true, provider: 'local' }],
  sessions: [{ session_id: 'abcdef12', title: '上次任务' }],
  skills: [{ name: 'review', description: '代码审查' }],
}

test('bare slash lists commands', () => {
  const items = slashSuggestions('/', context)
  assert.equal(items[0].kind, 'command')
  assert.ok(items.some(item => item.value === '/model'))
})

test('selecting a submenu command fills instead of sending', () => {
  const result = applySuggestion('/', { value: '/model', detail: '', kind: 'command' })
  assert.equal(result.kind, 'fill')
  assert.equal(result.text, '/model ')
})

test('model command without space opens values', () => {
  const items = slashSuggestions('/model', context)
  assert.equal(items[0].kind, 'value')
  assert.equal(items[0].value, 'fake')
  const result = applySuggestion('/model', items[0])
  assert.equal(result.kind, 'send')
  assert.equal(result.text, '/model fake')
})

test('isSubmenu covers nested commands', () => {
  assert.equal(isSubmenu('/permissions'), true)
  assert.equal(isSubmenu('/help'), false)
})
