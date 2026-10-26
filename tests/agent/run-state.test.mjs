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
