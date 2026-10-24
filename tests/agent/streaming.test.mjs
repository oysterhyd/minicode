import assert from 'node:assert/strict'
import {test} from 'node:test'
import {fixture,expectSequence,until} from './helpers.mjs'
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
// R7 / P1: provider streams are bounded by an idle timer, not a total timer.
const idle={MINICODE_PROVIDER_IDLE_TIMEOUT_S:'1'}
test('a stream longer than the idle timeout completes while bytes keep arriving',async t=>{
  const f=await fixture(t,[],idle);const mock=await sseMock(t,[{frames:anthropicFrames({text:'x'.repeat(30),chunks:30}),intervalMs:150}])
  const r=await f.run({model:await configureService(f,mock.url)})
  assert.equal(r.result.exit_reason,'completed');assert.equal(r.events.filter(delta).length,30)
})
test('silence after visible output fails as an idle timeout without a retry',async t=>{
  const f=await fixture(t,[],idle);const mock=await sseMock(t,[{frames:anthropicFrames({text:'partial'}).slice(0,3),hang:true}])
  const started=Date.now();const r=await f.run({model:await configureService(f,mock.url)})
  assert.equal(r.result.exit_reason,'provider_error');assert.match(r.result.error,/idle timeout/);assert.equal(mock.payloads.length,1);assert.ok(Date.now()-started<10000)
})
test('silence before any byte is an idle timeout that is retried',async t=>{
  const f=await fixture(t,[],idle);const mock=await sseMock(t,[{hang:true},{hang:true},{frames:anthropicFrames({text:'third attempt'})}])
  const r=await f.run({model:await configureService(f,mock.url)})
  assert.equal(r.result.exit_reason,'completed');assert.equal(mock.payloads.length,3)
  assert.ok(r.detail.events.filter(e=>e.type==='provider_retry').every(e=>/idle timeout/.test(e.data.error)))
})
test('the turn deadline still bounds a slow stream and is reported separately',async t=>{
  const f=await fixture(t);const mock=await sseMock(t,[{frames:anthropicFrames({text:'y'.repeat(40),chunks:40}),intervalMs:100}])
  await f.request('setBudget',{max_rounds:0,max_total_tokens:0,max_seconds:1.5});const r=await f.run({model:await configureService(f,mock.url)})
  assert.equal(r.result.exit_reason,'time_budget');assert.doesNotMatch(r.result.error,/idle/)
})
// Acceptance: a 200 s stream (one delta every 2 s) outlives the old 180 s cap.
// Opt in with MINICODE_SLOW_TESTS=1.
test('a 200 second paced stream completes',{skip:process.env.MINICODE_SLOW_TESTS!=='1',timeout:260000},async t=>{
  const f=await fixture(t);const mock=await sseMock(t,[{frames:anthropicFrames({text:'z'.repeat(100),chunks:100}),intervalMs:2000}])
  const from=f.events.length,started=await f.request('sendPrompt',{text:'long',workspace:f.workspace,model:await configureService(f,mock.url)})
  const terminal=e=>(e.event==='run_done'||e.event==='run_error')&&e.sessionId===started.sessionId
  assert.ok(await until(()=>f.events.slice(from).some(terminal),250000));const done=f.events.slice(from).find(terminal)
  assert.equal(done.event,'run_done');assert.equal(done.result.exit_reason,'completed',done.result.error);assert.equal(f.events.slice(from).filter(delta).length,100)
})
