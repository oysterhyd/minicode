import assert from 'node:assert/strict'
import net from 'node:net'
import {test} from 'node:test'
import {fixture,expectSequence,until} from './helpers.mjs'
import {sseMock,anthropicFrames,openaiFrames,configureService} from './sse-mock.mjs'
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
test('HTTP errors retain a bounded server message without retrying a bad request',async t=>{
  const f=await fixture(t);const body=JSON.stringify({error:{code:'context_length_exceeded',message:'The prompt exceeds this model context'},padding:'x'.repeat(8000)})
  const mock=await sseMock(t,[{status:400,body}]);const r=await f.run({model:await configureService(f,mock.url)})
  assert.equal(r.result.exit_reason,'provider_error');assert.equal(mock.payloads.length,1)
  assert.match(r.result.error,/context_length_exceeded/);assert.match(r.result.error,/The prompt exceeds this model context/)
  assert.ok(Buffer.byteLength(r.result.error)<4400,'response body must be bounded')
})
test('HTTP retry metadata preserves Retry-After even when it is zero',async t=>{
  const f=await fixture(t);const mock=await sseMock(t,[{status:429,retryAfter:0,body:'{"error":{"message":"slow down"}}'},{frames:anthropicFrames({text:'ok'})}])
  const r=await f.run({model:await configureService(f,mock.url)})
  assert.equal(r.result.exit_reason,'completed');const retry=r.detail.events.find(e=>e.type==='provider_retry')
  assert.equal(retry.data.retry_after_s,0);assert.match(retry.data.error,/slow down/)
})
test('usage stays available after a failed attempt and a measured response',async t=>{
  const f=await fixture(t);const mock=await sseMock(t,[{status:429,retryAfter:0},{frames:anthropicFrames({text:'measured',input:13,output:4})}])
  const r=await f.run({model:await configureService(f,mock.url)})
  assert.equal(r.result.exit_reason,'completed');assert.equal(r.result.total_usage.available,true);assert.equal(r.result.total_usage.input_tokens,13);assert.equal(r.detail.summary.usage_available,true)
})
test('mixed missing and present usage remains available and is persisted',async t=>{
  const f=await fixture(t);const first=anthropicFrames({tools:[{id:'read-usage',name:'read',arguments:{path:'README.md'}}]})
  delete first[0].message.usage;delete first.at(-2).usage
  const mock=await sseMock(t,[{frames:first},{frames:anthropicFrames({text:'done',input:21,output:6})}])
  const r=await f.run({model:await configureService(f,mock.url)})
  assert.equal(r.result.exit_reason,'completed');assert.equal(mock.payloads.length,2);assert.equal(r.result.total_usage.available,true);assert.equal(r.result.total_usage.input_tokens,21);assert.equal(r.detail.summary.usage_available,true)
})
test('empty usage objects do not count as measured usage',async t=>{
  for(const style of ['anthropic','openai']){
    const f=await fixture(t);const frames=style==='anthropic'?anthropicFrames({text:'done'}):openaiFrames({text:'done'})
    if(style==='anthropic'){frames[0].message.usage={};frames.at(-2).usage={}}else frames.at(-1).usage={}
    const mock=await sseMock(t,[{frames,done:style==='openai'}]);const r=await f.run({model:await configureService(f,mock.url,style)})
    assert.equal(r.result.exit_reason,'completed');assert.equal(r.result.total_usage.available,false,style);assert.equal(r.detail.summary.usage_available,false)
  }
})
test('Anthropic cache breakpoints preserve stable prefixes and cache usage',async t=>{
  const f=await fixture(t,[],{MINICODE_ANTHROPIC_PROMPT_CACHE:'on'});const first=anthropicFrames({tools:[{id:'read-cache',name:'read',arguments:{path:'README.md'}}]})
  const second=anthropicFrames({text:'cached',input:12,output:4});second[0].message.usage.cache_read_input_tokens=7;second[0].message.usage.cache_creation_input_tokens=2
  const mock=await sseMock(t,[{frames:first},{frames:second}]);const r=await f.run({model:await configureService(f,mock.url)})
  assert.equal(r.result.exit_reason,'completed');assert.equal(mock.payloads.length,2)
  const [a,b]=mock.payloads;assert.equal(JSON.stringify(a.system),JSON.stringify(b.system));assert.equal(JSON.stringify(a.tools),JSON.stringify(b.tools))
  assert.equal(b.system.at(-1).cache_control.type,'ephemeral');assert.equal(b.tools.at(-1).cache_control.type,'ephemeral')
  for(const message of b.messages.slice(-2))assert.equal(message.content.at(-1).cache_control.type,'ephemeral')
  const markers=[...b.system,...b.tools,...b.messages.flatMap(m=>m.content)].filter(block=>block.cache_control)
  assert.equal(markers.length,4);assert.equal(r.result.total_usage.cache_read_tokens,7);assert.equal(r.result.total_usage.cache_write_tokens,2);assert.equal(r.detail.summary.cache_read_tokens,7)
})
test('Anthropic cache markers can be disabled for incompatible gateways',async t=>{
  const f=await fixture(t,[],{MINICODE_ANTHROPIC_PROMPT_CACHE:'off'});const mock=await sseMock(t,[{frames:anthropicFrames({text:'without cache'})}])
  const r=await f.run({model:await configureService(f,mock.url)});assert.equal(r.result.exit_reason,'completed')
  assert.equal(typeof mock.payloads[0].system,'string');assert.ok(!JSON.stringify(mock.payloads[0]).includes('"cache_control"'))
})
test('OpenAI-compatible payloads do not receive Anthropic cache fields',async t=>{
  const f=await fixture(t);const mock=await sseMock(t,[{frames:openaiFrames({text:'openai'}),done:true}])
  const r=await f.run({model:await configureService(f,mock.url,'openai')});assert.equal(r.result.exit_reason,'completed')
  assert.ok(!JSON.stringify(mock.payloads[0]).includes('"cache_control"'))
})
test('connection failures include the underlying socket error without credentials',async t=>{
  const reservation=net.createServer();await new Promise(resolve=>reservation.listen(0,'127.0.0.1',resolve))
  const port=reservation.address().port;await new Promise(resolve=>reservation.close(resolve))
  const f=await fixture(t,[],{MINICODE_PROVIDER_MAX_RETRIES:'0'});const r=await f.run({model:await configureService(f,`http://127.0.0.1:${port}`)})
  assert.equal(r.result.exit_reason,'provider_error');assert.match(r.result.error,/provider connection failed/)
  assert.match(r.result.error,/connect|refused|连接/i);assert.doesNotMatch(r.result.error,/local-mock-key/)
})
test('a disconnected proxy reports its underlying connection failure',async t=>{
  const reservation=net.createServer();await new Promise(resolve=>reservation.listen(0,'127.0.0.1',resolve))
  const proxy=`http://127.0.0.1:${reservation.address().port}`;await new Promise(resolve=>reservation.close(resolve))
  const f=await fixture(t,[],{HTTP_PROXY:proxy,http_proxy:proxy,HTTPS_PROXY:proxy,https_proxy:proxy,NO_PROXY:'',no_proxy:'',MINICODE_PROVIDER_MAX_RETRIES:'0'})
  const r=await f.run({model:await configureService(f,'http://provider.invalid')})
  assert.equal(r.result.exit_reason,'provider_error');assert.match(r.result.error,/provider connection failed/)
  assert.match(r.result.error,/10061|refused|拒绝/i);assert.doesNotMatch(r.result.error,/local-mock-key/)
})
test('Anthropic overloaded SSE errors retain details and retry',async t=>{
  const f=await fixture(t);const mock=await sseMock(t,[{frames:[{type:'error',error:{type:'overloaded_error',message:'temporarily overloaded'}}]},{frames:anthropicFrames({text:'recovered'})}])
  const r=await f.run({model:await configureService(f,mock.url)})
  assert.equal(r.result.exit_reason,'completed');assert.equal(mock.payloads.length,2)
  const retry=r.detail.events.find(e=>e.type==='provider_retry');assert.match(retry.data.error,/overloaded_error/);assert.match(retry.data.error,/temporarily overloaded/)
})
test('named SSE errors and OpenAI error objects are classified',async t=>{
  for(const [style,step,success] of [
    ['anthropic',{eventNames:['error'],frames:[{type:'rate_limit_error',message:'anthropic limit'}]},anthropicFrames({text:'ok'})],
    ['openai',{frames:[{error:{type:'api_error',message:'gateway busy'}}]},openaiFrames({text:'ok'})],
  ]){
    const f=await fixture(t);const mock=await sseMock(t,[step,{frames:success,done:style==='openai'}])
    const r=await f.run({model:await configureService(f,mock.url,style)});assert.equal(r.result.exit_reason,'completed');assert.equal(mock.payloads.length,2)
    assert.match(r.detail.events.find(e=>e.type==='provider_retry').data.error,style==='anthropic'?/anthropic limit/:/gateway busy/)
  }
})
test('invalid SSE requests fail once and include the server message',async t=>{
  const f=await fixture(t);const mock=await sseMock(t,[{frames:[{type:'error',error:{type:'invalid_request_error',message:'unknown parameter'}}]}])
  const r=await f.run({model:await configureService(f,mock.url)})
  assert.equal(r.result.exit_reason,'provider_error');assert.equal(mock.payloads.length,1);assert.match(r.result.error,/invalid_request_error/);assert.match(r.result.error,/unknown parameter/)
})
test('529 respects Retry-After before retrying',async t=>{
  const f=await fixture(t);const mock=await sseMock(t,[{status:529,retryAfter:5,body:'{"error":{"type":"overloaded_error","message":"busy"}}'},{frames:anthropicFrames({text:'recovered'})}])
  const model=await configureService(f,mock.url);const started=Date.now();const r=await f.run({model});const elapsed=Date.now()-started
  assert.equal(r.result.exit_reason,'completed');assert.equal(mock.payloads.length,2);assert.ok(elapsed>=4700&&elapsed<12000,`elapsed ${elapsed}ms`)
  assert.equal(r.detail.events.find(e=>e.type==='provider_retry').data.delay_s,5)
})
test('the default policy allows more than two retries',async t=>{
  const f=await fixture(t);const mock=await sseMock(t,[...Array.from({length:4},()=>({status:429,retryAfter:0})),{frames:anthropicFrames({text:'finally'})}])
  const r=await f.run({model:await configureService(f,mock.url)})
  assert.equal(r.result.exit_reason,'completed');assert.equal(mock.payloads.length,5);assert.equal(r.detail.events.filter(e=>e.type==='provider_retry').length,4)
})
test('the default policy stops after six retries',async t=>{
  const f=await fixture(t);const mock=await sseMock(t,[{status:429,retryAfter:0}]);const r=await f.run({model:await configureService(f,mock.url)})
  assert.equal(r.result.exit_reason,'provider_error');assert.equal(mock.payloads.length,7);assert.equal(r.detail.events.filter(e=>e.type==='provider_retry').length,6)
})
test('partial streams are discarded before replay after disconnect or overload',async t=>{
  const partial=anthropicFrames({text:'discard this partial'})
  for(const first of [
    {frames:partial,dropAfter:3},
    {frames:[...partial.slice(0,3),{type:'error',error:{type:'overloaded_error',message:'busy mid-stream'}}]},
  ]){
    const f=await fixture(t);const mock=await sseMock(t,[first,{frames:anthropicFrames({text:'complete answer'})}]);const r=await f.run({model:await configureService(f,mock.url)})
    assert.equal(r.result.exit_reason,'completed');assert.equal(mock.payloads.length,2)
    const retry=r.events.find(item('provider_retry'));assert.equal(retry.item.data.reset_output,true)
    let visible='';for(const e of r.events){if(delta(e))visible+=e.text;if(item('provider_retry')(e)&&e.item.data.reset_output)visible=''}
    assert.equal(visible,'complete answer');assert.ok(!JSON.stringify(r.detail.messages).includes('discard this partial'))
  }
})
test('turn deadline interrupts Retry-After waiting',async t=>{
  const f=await fixture(t);const mock=await sseMock(t,[{status:529,retryAfter:5}]);const model=await configureService(f,mock.url)
  await f.request('setBudget',{max_rounds:0,max_total_tokens:0,max_seconds:1});const started=Date.now();const r=await f.run({model})
  assert.equal(r.result.exit_reason,'time_budget');assert.equal(mock.payloads.length,1);assert.ok(Date.now()-started<3500)
})
test('cancelling interrupts Retry-After waiting',async t=>{
  const f=await fixture(t);const mock=await sseMock(t,[{status:529,retryAfter:5}]);const model=await configureService(f,mock.url)
  const from=f.events.length;const started=await f.request('sendPrompt',{workspace:f.workspace,text:'cancel waiting',model})
  await f.wait(e=>item('provider_retry')(e)&&e.sessionId===started.sessionId,from)
  const begin=Date.now();await f.request('cancelTurn',{sessionId:started.sessionId})
  const done=await f.wait(e=>e.event==='run_done'&&e.sessionId===started.sessionId,from)
  assert.equal(done.result.exit_reason,'cancelled');assert.equal(mock.payloads.length,1);assert.ok(Date.now()-begin<2000)
})
test('scaffold: a stream dropped after visible output is retried',async t=>{
  const f=await fixture(t);const mock=await sseMock(t,[{frames:anthropicFrames({text:'partial answer',chunks:2}),dropAfter:3},{frames:anthropicFrames({text:'replacement'})}])
  const r=await f.run({model:await configureService(f,mock.url)})
  assert.equal(r.result.exit_reason,'completed');assert.equal(mock.payloads.length,2);assert.ok(r.events.some(e=>item('provider_retry')(e)&&e.item.data.reset_output))
})
// R7 / P1: provider streams are bounded by an idle timer, not a total timer.
const idle={MINICODE_PROVIDER_IDLE_TIMEOUT_S:'1',MINICODE_PROVIDER_RETRY_BASE_S:'0.01',MINICODE_PROVIDER_RETRY_MAX_S:'0.02'}
test('a stream longer than the idle timeout completes while bytes keep arriving',async t=>{
  const f=await fixture(t,[],idle);const mock=await sseMock(t,[{frames:anthropicFrames({text:'x'.repeat(30),chunks:30}),intervalMs:150}])
  const r=await f.run({model:await configureService(f,mock.url)})
  assert.equal(r.result.exit_reason,'completed');assert.equal(r.events.filter(delta).length,30)
})
test('silence after visible output retries and clears the partial answer',async t=>{
  const f=await fixture(t,[],idle);const mock=await sseMock(t,[{frames:anthropicFrames({text:'partial'}).slice(0,3),hang:true},{frames:anthropicFrames({text:'replacement'})}])
  const started=Date.now();const r=await f.run({model:await configureService(f,mock.url)})
  const retry=r.detail.events.find(e=>e.type==='provider_retry');assert.equal(r.result.exit_reason,'completed');assert.match(retry.data.error,/idle timeout/);assert.equal(retry.data.reset_output,true);assert.equal(mock.payloads.length,2);assert.ok(Date.now()-started<10000)
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
