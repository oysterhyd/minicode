import assert from 'node:assert/strict'
import {test} from 'node:test'
import {fixture} from './helpers.mjs'

test('D1: approval requests include the call identity used by their decisions',async t=>{
 const f=await fixture(t,[{tool_calls:[{name:'write',arguments:{path:'approval.txt',content:'test'}}]},{text:'done'}])
 const started=await f.request('sendPrompt',{workspace:f.workspace,model:'fake',text:'approval'})
 const approval=await f.wait(e=>e.event==='approval')
 const request=f.events.find(e=>e.event==='agent_event'&&e.item.type==='approval_request')
 assert.equal(approval.request.call_id,request.item.data.call_id)
 await f.request('resolveApproval',{sessionId:started.sessionId,approvalId:approval.approvalId,granted:false})
 const decision=await f.wait(e=>e.event==='agent_event'&&e.item.type==='approval_decision')
 assert.equal(decision.item.data.call_id,approval.request.call_id)
 await f.wait(e=>e.event==='run_done')
})
