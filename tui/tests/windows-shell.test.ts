import assert from 'node:assert/strict'
import test from 'node:test'
import { approvalOptions } from '../src/components/ApprovalBlock.js'
import { toolMeta } from '../src/tools.js'

test('PowerShell approvals identify the command tool', () => {
  assert.equal(toolMeta('powershell').done, 'PowerShell')
  assert.match(approvalOptions({ id: 'approval', kind: 'approval', name: 'powershell' })[1], /PowerShell/)
})
