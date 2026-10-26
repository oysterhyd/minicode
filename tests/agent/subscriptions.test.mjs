import assert from 'node:assert/strict'
import {test} from 'node:test'
import {fixture} from './helpers.mjs'
test('B22: joining a busy session receives a snapshot and subsequent stream without disconnecting its first view',async t=>{
 const f=await fixture(t,[{tool_calls:[{name:'write',arguments:{path:'pending.txt',content:'wait'}}]},{text:'both views see this'}])
 const first=await f.request('sendPrompt',{workspace:f.workspace,model:'fake',clientKey:'first',text:'start'})
 const approval=await f.wait(e=>e.event==='approval'&&e.sessionId===first.sessionId)
 const from=f.events.length
 const selected=await f.request('selectSession',{clientKey:'second',sessionId:first.sessionId,deferRuntime:true})
 assert.equal(selected.running,true)
 const replay=f.events.slice(from).find(e=>e.event==='run_started'&&e.clientKey==='second')
 assert.ok(replay,'new view did not receive its current run snapshot')
 assert.equal(replay.runId,first.runId);assert.equal(replay.state.running,true)
 await f.request('resolveApproval',{clientKey:'second',sessionId:first.sessionId,approvalId:approval.approvalId,granted:false})
 for(const key of ['first','second']) {
  await f.wait(e=>e.event==='run_done'&&e.clientKey===key&&e.runId===first.runId,from)
  assert.ok(f.events.slice(from).some(e=>e.event==='text_delta'&&e.clientKey===key))
 }
 await f.request('runSlash',{clientKey:'first',sessionId:first.sessionId,workspace:f.workspace,text:'/new'})
 assert.equal((await f.request('getState',{clientKey:'second',sessionId:first.sessionId})).sessionId,first.sessionId)
 assert.equal((await f.request('getState',{clientKey:'first'})).sessionId,null,'new view remained bound to old session')
})
