import assert from 'node:assert/strict'
import {test} from 'node:test'
import {fixture} from './helpers.mjs'

test('R33 child progress reaches parent in order and persists without sensitive child payloads',async t=>{
  const secret='PRIVATE_PROGRESS_TOKEN'
  const f=await fixture(t,[
    {tool_calls:[{id:'parent-delegate',name:'delegate',arguments:{kind:'explore',task:'inspect'}}]},
    {text:secret,tool_calls:[{id:'child-read',name:'read',arguments:{path:'README.md',unused:secret}}]},
    {text:JSON.stringify({summary:'done',findings:[],evidence_refs:['README.md'],unresolved:[]})},
    {text:'parent done'},
  ])
  const run=await f.run()
  const live=run.events.filter(e=>e.event==='agent_event'&&e.item.type==='subagent_progress')
  assert.ok(live.length>=3,'child progress must be forwarded while delegate is running')
  const events=run.detail.events
  const start=events.find(e=>e.type==='subagent_start')
  const progress=events.filter(e=>e.type==='subagent_progress')
  const end=events.find(e=>e.type==='subagent_result')
  assert.equal(start.data.call_id,'parent-delegate');assert.equal(end.data.call_id,'parent-delegate')
  assert.deepEqual(live.map(e=>e.item.seq),progress.map(e=>e.seq))
  assert.ok(progress.every(e=>e.seq>start.seq&&e.seq<end.seq&&e.data.call_id==='parent-delegate'&&e.data.child_session_id===start.data.child_session_id))
  const tool=progress.filter(e=>e.data.child_call_id==='child-read')
  assert.deepEqual(tool.map(e=>[e.data.name,e.data.status]),[['read','running'],['read','completed']])
  assert.ok(tool[0].data.child_event_seq<tool[1].data.child_event_seq)
  assert.ok(!JSON.stringify(progress).includes(secret));assert.ok(!JSON.stringify(progress).includes('README.md'))
})

test('R33 paused child has a terminal status before the delegate result',async t=>{
  const f=await fixture(t,[{tool_calls:[{id:'d',name:'delegate',arguments:{kind:'explore',task:'inspect'}}]},{tool_calls:[{name:'read',arguments:{path:'README.md'}}]}])
  await f.request('setBudget',{max_rounds:2,max_total_tokens:0,max_seconds:0})
  const run=await f.run()
  const child=run.detail.events.find(e=>e.type==='subagent_result')
  assert.equal(child.data.call_id,'d');assert.equal(child.data.exit_reason,'max_rounds')
  assert.ok(run.detail.events.some(e=>e.type==='subagent_progress'&&e.data.phase==='tool'))
})

test('R33 child failure reports a safe terminal status',async t=>{
  const f=await fixture(t,[{tool_calls:[{id:'d',name:'delegate',arguments:{kind:'explore',task:'inspect'}}]},{error:'PRIVATE_CHILD_ERROR'}])
  const run=await f.settle()
  const end=run.events.find(e=>e.event==='agent_event'&&e.item.type==='subagent_result')
  assert.equal(end.item.data.call_id,'d');assert.equal(end.item.data.exit_reason,'provider_error')
  assert.ok(!JSON.stringify(end).includes('PRIVATE_CHILD_ERROR'))
})
