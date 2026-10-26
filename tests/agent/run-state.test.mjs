import fs from 'node:fs'
import path from 'node:path'
import assert from 'node:assert/strict'
import {test} from 'node:test'
import {fixture} from './helpers.mjs'

test('B20: a registered run has one identity across response, state and all run events', async t=>{
  const f=await fixture(t,[{text:'first'}])
  let previous
  for(let i=0;i<2;i++) {
    const from=f.events.length
    const started=await f.request('sendPrompt',{workspace:f.workspace,model:'fake',text:`turn-${i}`})
    assert.equal(typeof started.runId,'string')
    assert.notEqual(started.runId,previous)
    const terminal=await f.wait(e=>(e.event==='run_done'||e.event==='run_error')&&e.runId===started.runId,from)
    assert.equal(terminal.event,'run_done')
    const events=f.events.slice(from).filter(e=>e.sessionId===started.sessionId)
    assert.equal(events[0].event,'run_started')
    assert.ok(events.every(e=>e.runId===started.runId))
    const state=await f.request('getState',{sessionId:started.sessionId})
    assert.equal(state.runId,started.runId)
    previous=started.runId
  }
})

test('D3: async acceptance returns a run identity before catalog scans and reports preparation failures',async t=>{
 const f=await fixture(t,[{text:'accepted'}])
 const started=await f.request('sendPrompt',{workspace:f.workspace,model:'fake',text:'missing attachment',attachments:[{asset_id:'missing'}],asyncStart:true})
 assert.equal(typeof started.runId,'string')
 const failed=await f.wait(e=>e.event==='run_error'&&e.runId===started.runId)
 assert.ok(failed.error)
 assert.equal((await f.request('getState')).running,false)
 const next=await f.request('sendPrompt',{workspace:f.workspace,model:'fake',text:'valid',asyncStart:true})
 assert.notEqual(next.runId,started.runId)
 await f.wait(e=>e.event==='run_done'&&e.runId===next.runId)
})

test('D3: accepted new sessions keep skills activated before the first prompt',async t=>{
 const f=await fixture(t,[{text:'complete'}]);const dir=path.join(f.workspace,'.minicode/skills/sticky');fs.mkdirSync(dir,{recursive:true});fs.writeFileSync(path.join(dir,'SKILL.md'),'---\nname: sticky\ndescription: fixture\n---\nKeep these rules\n')
 await f.request('setSkillActive',{workspace:f.workspace,name:'sticky',active:true})
 const started=await f.request('sendPrompt',{workspace:f.workspace,model:'fake',text:'with skill',asyncStart:true})
 await f.wait(e=>e.event==='run_done'&&e.runId===started.runId)
 const detail=await f.request('getSession',{sessionId:started.sessionId})
 assert.equal(typeof detail.events.find(e=>e.type==='session_start').data.active_skills.sticky,'string')
})
