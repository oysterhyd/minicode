import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import http from 'node:http'
import {test} from 'node:test'
import {fixture} from './helpers.mjs'
test('two read-only delegates overlap while sharing parent usage',async t=>{
  const f=await fixture(t);let parent=0,active=0,maxActive=0;const waiting=[]
  const url=await server(t,(req,res)=>{let body='';req.on('data',d=>body+=d);req.on('end',()=>{
    const p=JSON.parse(body),system=p.messages.find(m=>m.role==='system')?.content||''
    if(system.includes('你是子助手。')){active++;maxActive=Math.max(maxActive,active);const finish=()=>{if(res.writableEnded)return;active--;write(res,[{choices:[{delta:{content:JSON.stringify({summary:'done',findings:[],evidence_refs:[],unresolved:[]})},finish_reason:'stop'}],usage:{prompt_tokens:10,completion_tokens:5}}])};const fallback=setTimeout(finish,4000);waiting.push(()=>{clearTimeout(fallback);finish()});if(waiting.length===2)waiting.forEach(fn=>fn())}
    else if(parent++===0){write(res,[{choices:[{delta:{tool_calls:[{index:0,id:'delegate-a',function:{name:'delegate',arguments:JSON.stringify({kind:'explore',task:'inspect A'})}},{index:1,id:'delegate-b',function:{name:'delegate',arguments:JSON.stringify({kind:'explore',task:'inspect B'})}}]},finish_reason:'tool_calls'}],usage:{prompt_tokens:20,completion_tokens:10}}])}
    else write(res,[{choices:[{delta:{content:'completed'},finish_reason:'stop'}],usage:{prompt_tokens:20,completion_tokens:10}}])
  })})
  await f.request('setPermissionMode',{mode:'bypass'});const r=await f.run({model:await configure(f,url)})
  assert.equal(r.result.exit_reason,'completed');assert.equal(maxActive,2);assert.equal(r.result.total_usage.input_tokens,60);assert.equal(r.result.total_usage.output_tokens,30);assert.equal(r.detail.events.filter(e=>e.type==='subagent_result').length,2)
})
async function server(t,handler){const instance=http.createServer(handler);await new Promise(resolve=>instance.listen(0,'127.0.0.1',resolve));t.after(()=>new Promise(resolve=>instance.close(resolve)));return `http://127.0.0.1:${instance.address().port}`}
async function configure(f,url,style='openai',capabilities={}){const config=await f.request('saveService',{service:{name:'Local fixture',baseUrl:style==='openai'?url+'/v1':url,apiStyle:style,apiKey:'local-fixture-key',enabled:true,models:[{modelId:'fixture-model',name:'Fixture',contextWindow:200000,maxOutputTokens:8192,supportsEffort:true,capabilities}]}});return config.defaultModel}
function write(res,frames,done=true){res.writeHead(200,{'content-type':'text/event-stream'});for(const frame of frames){const bytes=Buffer.from(`data: ${JSON.stringify(frame)}\n\n`);for(let i=0;i<bytes.length;i+=7)res.write(bytes.subarray(i,i+7));}if(done)res.write('data: [DONE]\n\n');res.end()}
test('OpenAI SSE preserves Unicode, reasoning, cache usage and payload schemas',async t=>{const f=await fixture(t);let payload;const url=await server(t,(req,res)=>{let body='';req.on('data',d=>body+=d);req.on('end',()=>{payload=JSON.parse(body);assert.equal(req.headers.authorization,'Bearer local-fixture-key');write(res,[{choices:[{delta:{reasoning_content:'思考中'},finish_reason:null}]},{choices:[{delta:{content:'你好 Rust 🦀'},finish_reason:null}]},{choices:[{delta:{},finish_reason:'stop'}]},{choices:[],usage:{prompt_tokens:30,completion_tokens:7,prompt_tokens_details:{cached_tokens:10,cache_creation_tokens:5}}}])})});const model=await configure(f,url);const r=await f.run({model});assert.equal(r.result.exit_reason,'completed');assert.equal(r.result.total_usage.input_tokens,30);assert.equal(r.result.total_usage.cache_read_tokens,10);assert.equal(r.result.total_usage.cache_write_tokens,5);assert.equal(r.events.filter(e=>e.event==='text_delta').map(e=>e.text).join(''),'你好 Rust 🦀');assert.equal(r.events.filter(e=>e.event==='reasoning_delta').map(e=>e.text).join(''),'思考中');assert.equal(payload.stream_options.include_usage,true);assert.ok(payload.tools.some(t=>t.function.name==='read'));})
test('finish reason alone safely terminates an OpenAI stream',async t=>{const f=await fixture(t);const url=await server(t,(_req,res)=>write(res,[{choices:[{delta:{content:'completed without DONE'},finish_reason:'stop'}]}],false));const r=await f.run({model:await configure(f,url)});assert.equal(r.result.exit_reason,'completed');assert.equal(r.result.total_usage.available,false);})
test('429 retries before visible output but never repeats a partial response',async t=>{const f=await fixture(t);let count=0;const url=await server(t,(_req,res)=>{count++;if(count===1){res.writeHead(429);res.end('{}');return}write(res,[{choices:[{delta:{content:'after retry'},finish_reason:'stop'}]}])});const r=await f.run({model:await configure(f,url)});assert.equal(count,2);assert.equal(r.result.exit_reason,'completed');assert.ok(r.detail.events.some(e=>e.type==='provider_retry'));assert.equal(r.result.total_usage.available,false);})
test('OpenAI function argument fragments form one exact tool call',async t=>{const f=await fixture(t);let count=0;let payloads=[];const url=await server(t,(req,res)=>{let body='';req.on('data',d=>body+=d);req.on('end',()=>{payloads.push(JSON.parse(body));count++;if(count===1)write(res,[{choices:[{delta:{tool_calls:[{index:0,id:'call-one',function:{name:'read',arguments:'{"path":'}}]},finish_reason:null}]},{choices:[{delta:{tool_calls:[{index:0,function:{arguments:'"README.md"}'}}]},finish_reason:'tool_calls'}]}]);else write(res,[{choices:[{delta:{content:'read verified'},finish_reason:'stop'}]}])})});const r=await f.run({model:await configure(f,url)});assert.equal(r.result.exit_reason,'completed');assert.equal(count,2);const tool=payloads[1].messages.find(m=>m.role==='tool');assert.equal(tool.tool_call_id,'call-one');assert.ok(tool.content.includes('repository'));})
test('malformed provider arguments and reused tool ids cannot execute writes',async t=>{const f=await fixture(t);const url=await server(t,(_req,res)=>write(res,[{choices:[{delta:{tool_calls:[{index:0,id:'bad',function:{name:'write',arguments:'{"path":"unsafe",'}}]},finish_reason:'tool_calls'}]}]));await f.request('setPermissionMode',{mode:'bypass'});const r=await f.run({model:await configure(f,url)});assert.equal(r.result.exit_reason,'provider_error');assert.equal(fs.existsSync(path.join(f.workspace,'unsafe')),false);})
test('model list connection test redacts keys and preserves service metadata',async t=>{const f=await fixture(t);const url=await server(t,(_req,res)=>{res.setHeader('content-type','application/json');res.end(JSON.stringify({data:[{id:'model-one',display_name:'Model one'}]}))});const service={name:'Fixture',baseUrl:url+'/v1',apiStyle:'openai',apiKey:'fixture-only-key',models:[]};const result=await f.request('fetchServiceModels',{service});assert.equal(result.models[0].modelId,'model-one');assert.equal(result.models[0].name,'Model one');assert.ok(!JSON.stringify(result).includes('fixture-only-key'));})
test('Anthropic SSE keeps usage, tool schemas and image payloads',async t=>{const f=await fixture(t);let payload;const url=await server(t,(req,res)=>{let body='';req.on('data',d=>body+=d);req.on('end',()=>{payload=JSON.parse(body);assert.equal(req.url,'/v1/messages');assert.equal(req.headers['anthropic-version'],'2023-06-01');write(res,[{type:'message_start',message:{usage:{input_tokens:20,cache_read_input_tokens:6,cache_creation_input_tokens:2}}},{type:'content_block_start',index:0,content_block:{type:'text',text:''}},{type:'content_block_delta',index:0,delta:{type:'text_delta',text:'图片分析完成'}},{type:'message_delta',delta:{stop_reason:'end_turn'},usage:{output_tokens:8}},{type:'message_stop'}],false)})});const image=path.join(f.root,'one.png');fs.writeFileSync(image,Buffer.from('89504e470d0a1a0a','hex'));const attachments=await f.request('importAttachments',{paths:[image]});const r=await f.run({model:await configure(f,url,'anthropic',{images:true,pdf:true}),attachments});assert.equal(r.result.exit_reason,'completed');assert.equal(r.result.total_usage.input_tokens,28);assert.equal(r.result.total_usage.cache_write_tokens,2);assert.equal(payload.messages[0].content[0].type,'image');assert.equal(payload.messages[0].content[0].source.type,'base64');assert.ok(payload.tools.some(t=>t.name==='read'&&t.input_schema.type==='object'));})
test('cancellation of a waiting HTTP stream does not block getState',async t=>{const f=await fixture(t);let response;const url=await server(t,(_req,res)=>{response=res;res.writeHead(200,{'content-type':'text/event-stream'});res.write(': keepalive\n\n')});t.after(()=>response?.end());const model=await configure(f,url);const started=await f.request('sendPrompt',{model,workspace:f.workspace,text:'wait'});await new Promise(resolve=>setTimeout(resolve,80));const before=Date.now();await f.request('cancelTurn',{sessionId:started.sessionId});const done=await f.wait(e=>e.event==='run_done');assert.equal(done.result.exit_reason,'cancelled');assert.ok(Date.now()-before<2000);assert.equal((await f.request('getState',{sessionId:started.sessionId})).running,false);})

test('P5: Anthropic thinking, signature and redacted blocks survive a tool round trip',async t=>{
  const f=await fixture(t),payloads=[]
  const url=await server(t,(req,res)=>{let body='';req.on('data',d=>body+=d);req.on('end',()=>{
    payloads.push(JSON.parse(body))
    if(payloads.length===1)write(res,[
      {type:'message_start',message:{usage:{input_tokens:10}}},
      {type:'content_block_start',index:0,content_block:{type:'thinking',thinking:''}},
      {type:'content_block_delta',index:0,delta:{type:'thinking_delta',thinking:'first thought'}},
      {type:'content_block_delta',index:0,delta:{type:'signature_delta',signature:'sig-part-'}},
      {type:'content_block_delta',index:0,delta:{type:'signature_delta',signature:'two'}},
      {type:'content_block_start',index:1,content_block:{type:'redacted_thinking',data:'encrypted'}},
      {type:'content_block_start',index:2,content_block:{type:'tool_use',id:'call_think',name:'read',input:{}}},
      {type:'content_block_delta',index:2,delta:{type:'input_json_delta',partial_json:'{"path":"README.md"}'}},
      {type:'message_delta',delta:{stop_reason:'tool_use'},usage:{output_tokens:12}},{type:'message_stop'}
    ],false)
    else write(res,[{type:'content_block_start',index:0,content_block:{type:'text',text:''}},{type:'content_block_delta',index:0,delta:{type:'text_delta',text:'done'}},{type:'message_delta',delta:{stop_reason:'end_turn'}},{type:'message_stop'}],false)
  })})
  const result=await f.run({model:await configure(f,url,'anthropic')})
  assert.equal(result.result.exit_reason,'completed')
  const stored=result.detail.messages.find(m=>m.role==='assistant')
  assert.equal(stored.content[0].type,'thinking')
  assert.equal(stored.content[0].thinking,'first thought')
  assert.equal(stored.content[0].signature,'sig-part-two')
  assert.equal(stored.content[1].type,'redacted_thinking')
  assert.equal(stored.content[1].data,'encrypted')
  const assistant=payloads[1].messages.find(m=>m.role==='assistant')
  assert.deepEqual(assistant.content.slice(0,2),[{type:'thinking',thinking:'first thought',signature:'sig-part-two'},{type:'redacted_thinking',data:'encrypted'}])
  assert.equal(assistant.content[2].type,'tool_use')
  assert.ok(result.events.some(e=>e.event==='reasoning_delta'&&e.text==='first thought'))
})

test('P5: compatible reasoning_content is persisted and replayed during tool use',async t=>{
  const f=await fixture(t),payloads=[]
  const url=await server(t,(req,res)=>{let body='';req.on('data',d=>body+=d);req.on('end',()=>{
    payloads.push(JSON.parse(body))
    if(payloads.length===1)write(res,[{choices:[{delta:{reasoning_content:'consider carefully'},finish_reason:null}]},{choices:[{delta:{tool_calls:[{index:0,id:'call_reason',function:{name:'read',arguments:'{"path":"README.md"}'}}]},finish_reason:'tool_calls'}]}])
    else write(res,[{choices:[{delta:{content:'done'},finish_reason:'stop'}]}])
  })})
  const config=await f.request('saveService',{service:{name:'DeepSeek mock',baseUrl:url+'/v1',apiStyle:'openai',apiKey:'key',enabled:true,models:[{modelId:'deepseek-reasoner',contextWindow:128000,maxOutputTokens:8192,supportsEffort:true}]}})
  const result=await f.run({model:config.defaultModel})
  assert.equal(result.result.exit_reason,'completed')
  assert.equal(result.detail.messages.find(m=>m.role==='assistant').content[0].thinking,'consider carefully')
  assert.equal(payloads[1].messages.find(m=>m.role==='assistant').reasoning_content,'consider carefully')
  assert.equal(payloads[1].messages.find(m=>m.role==='assistant').content,'')
})

test('P5: non-streaming Anthropic thinking blocks round-trip through saved history',async t=>{
  const f=await fixture(t),payloads=[]
  const url=await server(t,(req,res)=>{let body='';req.on('data',d=>body+=d);req.on('end',()=>{
    payloads.push(JSON.parse(body));res.setHeader('content-type','application/json')
    if(payloads.length===1)res.end(JSON.stringify({content:[{type:'thinking',thinking:'nonstream thought',signature:'sig'},{type:'redacted_thinking',data:'opaque'},{type:'tool_use',id:'tool_json',name:'read',input:{path:'README.md'}}],stop_reason:'tool_use'}))
    else res.end(JSON.stringify({content:[{type:'text',text:'done'}],stop_reason:'end_turn'}))
  })})
  const result=await f.run({model:await configure(f,url,'anthropic')})
  assert.equal(result.result.exit_reason,'completed')
  const assistant=payloads[1].messages.find(m=>m.role==='assistant')
  assert.deepEqual(assistant.content.slice(0,2),[{type:'thinking',thinking:'nonstream thought',signature:'sig'},{type:'redacted_thinking',data:'opaque'}])
  assert.ok(result.events.some(e=>e.event==='reasoning_delta'&&e.text==='nonstream thought'))
})
