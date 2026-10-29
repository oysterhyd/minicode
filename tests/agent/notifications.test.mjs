import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import {test} from 'node:test'
import {fixture} from './helpers.mjs'
import {sseMock,anthropicFrames,configureService} from './sse-mock.mjs'

test('R-N: acceptance feedback reaches the model as a reminder with normalized roles', async t => {
  const f = await fixture(t)
  const spec = path.join(f.root,'acceptance.json')
  fs.writeFileSync(spec,JSON.stringify({max_fix_attempts:2,items:[{id:'exists',type:'artifact',path:'missing.txt'}]}))
  await f.request('setAcceptance',{path:spec})
  const mock = await sseMock(t,[{frames:anthropicFrames({text:'premature finish'})}])
  const r = await f.run({model:await configureService(f,mock.url)})
  assert.equal(r.result.exit_reason,'goal_not_met')
  const messages = mock.payloads[1].messages
  const reminder = messages.flatMap(m => m.content).find(b => b.type === 'text' && b.text.includes('验收尚未通过'))
  assert.ok(reminder.text.startsWith('<system-reminder>'))
  assert.ok(reminder.text.endsWith('</system-reminder>'))
  for(let i=1;i<messages.length;i++) assert.notEqual(messages[i-1].role,messages[i].role)
})
