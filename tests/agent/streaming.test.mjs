import assert from 'node:assert/strict'
import {test} from 'node:test'
import {fixture,expectSequence} from './helpers.mjs'
import {sseMock,anthropicFrames,configureService} from './sse-mock.mjs'
const delta=e=>e.event==='text_delta',item=type=>e=>e.event==='agent_event'&&e.item.type===type
test('scaffold: paced SSE mock streams through the bridge in order',async t=>{
  const f=await fixture(t);const mock=await sseMock(t,[{frames:anthropicFrames({text:'paced answer',chunks:4}),intervalMs:20}])
  const r=await f.run({model:await configureService(f,mock.url)})
  assert.equal(r.result.exit_reason,'completed');assert.equal(r.events.filter(delta).map(e=>e.text).join(''),'paced answer')
  expectSequence(r.events,[item('round_start'),delta,item('assistant_message'),item('session_end'),e=>e.event==='run_done'])
  assert.equal(mock.payloads[0].messages[0].role,'user')
})
test('scaffold: status codes with Retry-After are retried before output',async t=>{
  const f=await fixture(t);const mock=await sseMock(t,[{status:429,retryAfter:0},{status:503},{frames:anthropicFrames({text:'after retries'})}])
  const r=await f.run({model:await configureService(f,mock.url)})
  assert.equal(r.result.exit_reason,'completed');assert.equal(mock.payloads.length,3);assert.equal(r.detail.events.filter(e=>e.type==='provider_retry').length,2)
})
test('scaffold: a stream dropped after visible output fails without a retry',async t=>{
  const f=await fixture(t);const mock=await sseMock(t,[{frames:anthropicFrames({text:'partial answer',chunks:2}),dropAfter:3}])
  const r=await f.run({model:await configureService(f,mock.url)})
  assert.equal(r.result.exit_reason,'provider_error');assert.equal(mock.payloads.length,1);assert.equal(r.events.filter(delta).map(e=>e.text).join(''),'partial')
})
