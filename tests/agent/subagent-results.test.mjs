import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import {test} from 'node:test'
import {fixture} from './helpers.mjs'

const payload={summary:'structured result',findings:['verified shape'],evidence_refs:[],unresolved:[]}
const delegate={tool_calls:[{name:'delegate',arguments:{kind:'explore',task:'report the result'}}]}
const outcome=run=>run.detail.events.filter(e=>e.type==='tool_call_result'&&e.data.name==='delegate').at(-1).data
for(const [name,text] of [
  ['bare JSON',JSON.stringify(payload)],
  ['JSON fence','```json\n'+JSON.stringify(payload)+'\n```'],
  ['surrounding prose','Here is the result:\n'+JSON.stringify(payload)+'\nEnd of report.'],
])test(`R31 child accepts ${name}`,async t=>{
  const f=await fixture(t,[delegate,{text},{text:'parent done'}]),run=await f.run()
  const result=outcome(run);assert.equal(result.success,true,result.error)
  assert.equal(JSON.parse(result.output_detail).summary,payload.summary)
  assert.equal(run.detail.events.find(e=>e.type==='subagent_result').data.structured,true)
})

test('R31 submit_result finishes the child without consuming the parent final answer',async t=>{
  const f=await fixture(t,[delegate,{tool_calls:[{name:'submit_result',arguments:payload}]},{text:'parent done'}]),run=await f.run()
  const result=outcome(run);assert.equal(result.success,true,result.error)
  assert.equal(JSON.parse(result.output_detail).summary,payload.summary)
  assert.equal(run.result.total_usage.input_tokens,300)
  assert.equal(run.detail.messages.filter(m=>m.role==='assistant').at(-1).content[0].text,'parent done')
  const id=run.detail.events.find(e=>e.type==='subagent_start').data.child_session_id
  const child=await f.request('getSession',{sessionId:id})
  assert.equal(child.events.filter(e=>e.type==='assistant_message').length,1)
  assert.equal(child.events.filter(e=>e.type==='subagent_result_submitted').length,1)
})

test('R31 rejects invalid result structure and an unrelated nested JSON object',async t=>{
  for(const text of [JSON.stringify({...payload,findings:[42]}),JSON.stringify({example:payload}),'{"unrelated":true}']) {
    const f=await fixture(t,[delegate,{text},{text:'parent done'}]),run=await f.run(),result=outcome(run)
    assert.equal(result.success,false);assert.equal(JSON.parse(result.output_detail).status,'invalid_result')
  }
})

test('R31 invalid submit_result arguments can be corrected in the same child',async t=>{
  const f=await fixture(t,[delegate,
    {tool_calls:[{name:'submit_result',arguments:{...payload,summary:42}}]},
    {tool_calls:[{name:'submit_result',arguments:{...payload,path:'not-observed.txt'}}]},
    {tool_calls:[{name:'submit_result',arguments:payload}]},{text:'parent done'}]),run=await f.run()
  assert.equal(outcome(run).success,true)
  const id=run.detail.events.find(e=>e.type==='subagent_start').data.child_session_id
  const child=await f.request('getSession',{sessionId:id}),results=child.events.filter(e=>e.type==='tool_call_result')
  assert.equal(results[0].data.success,false);assert.match(results[0].data.error,/invalid tool arguments/)
  assert.equal(results[1].data.success,false);assert.match(results[1].data.error,/invalid tool arguments/)
  assert.equal(results[2].data.success,true)
})

test('R31 submit_result is unavailable to a parent session',async t=>{
  const f=await fixture(t,[{tool_calls:[{name:'submit_result',arguments:payload}]},{text:'done'}]),run=await f.run()
  assert.equal(run.detail.events.find(e=>e.type==='tool_call_result').data.success,false)
  assert.equal(run.detail.events.some(e=>e.type==='subagent_result_submitted'),false)
})

test('R31 submitting a final result prevents later calls in the batch from writing',async t=>{
  const f=await fixture(t,[{tool_calls:[{name:'delegate',arguments:{kind:'fixer',task:'submit only'}}]},
    {tool_calls:[{name:'submit_result',arguments:payload},{name:'write',arguments:{path:'after-result.txt',content:'must not execute'}}]},
    {text:'parent done'}])
  await f.request('setPermissionMode',{mode:'bypass'});const run=await f.run()
  assert.equal(outcome(run).success,true);assert.equal(fs.existsSync(path.join(f.workspace,'after-result.txt')),false)
  const id=run.detail.events.find(e=>e.type==='subagent_start').data.child_session_id
  const child=await f.request('getSession',{sessionId:id})
  const skipped=child.events.find(e=>e.type==='tool_call_result'&&e.data.name==='write')
  assert.equal(skipped.data.success,false);assert.match(skipped.data.error,/未执行/)
})
