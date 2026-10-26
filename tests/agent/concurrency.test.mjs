import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import {test} from 'node:test'
import {setTimeout as delay} from 'node:timers/promises'
import {fixture} from './helpers.mjs'

test('B5/B15: concurrent catalog scans cannot delay another sessions cancellation by 100ms', async t => {
  const f = await fixture(t,[{tool_calls:[{name:'write',arguments:{path:'needs-approval.txt',content:'pending'}}]}])
  const started = await f.request('sendPrompt',{clientKey:'active',workspace:f.workspace,model:'fake',text:'wait for approval'})
  await f.wait(e=>e.event==='approval')
  const workspace=path.join(f.root,'slow-repo'),plugin=path.join(workspace,'.minicode/plugins/dense')
  fs.mkdirSync(plugin,{recursive:true})
  fs.writeFileSync(path.join(plugin,'plugin.json'),JSON.stringify({name:'dense',version:'1.0.0',minicode_version:'3.0.2',enabled:true}))
  const content=Buffer.alloc(30_000,'x')
  for(let i=0;i<500;i++)fs.writeFileSync(path.join(plugin,`${i}.txt`),content)
  let completed=0
  const scans=['slow-a','slow-b'].map(clientKey=>f.request('getCapabilities',{workspace,clientKey}).then(()=>{completed++}))
  t.after(()=>Promise.all(scans))
  await delay(20)
  assert.equal(completed,0,'scan fixture did not overlap cancellation')
  const start=performance.now()
  assert.equal(await f.request('cancelTurn',{sessionId:started.sessionId}),true)
  const ms=performance.now()-start
  t.diagnostic(`cancel acknowledgement: ${ms.toFixed(1)}ms`)
  assert.ok(ms<100,`cancellation waited ${ms.toFixed(1)}ms for catalog scans`)
  await Promise.all(scans)
  assert.equal((await f.wait(e=>e.event==='run_done'&&e.sessionId===started.sessionId)).result.exit_reason,'cancelled')
})
