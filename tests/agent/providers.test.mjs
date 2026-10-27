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
test('malformed provider arguments cannot execute writes',async t=>{const f=await fixture(t);let count=0;const url=await server(t,(_req,res)=>{if(count++===0)write(res,[{choices:[{delta:{tool_calls:[{index:0,id:'bad',function:{name:'write',arguments:'{"path":"unsafe",'}}]},finish_reason:'tool_calls'}]}]);else write(res,[{choices:[{delta:{content:'recovered'},finish_reason:'stop'}]}])});await f.request('setPermissionMode',{mode:'bypass'});const r=await f.run({model:await configure(f,url)});assert.equal(r.result.exit_reason,'completed');assert.equal(fs.existsSync(path.join(f.workspace,'unsafe')),false);})
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

test('C4: Anthropic effort can be enabled and signed thinking survives the enabled tool loop',async t=>{
  const f=await fixture(t),payloads=[]
  const url=await server(t,(req,res)=>{let body='';req.on('data',d=>body+=d);req.on('end',()=>{
    payloads.push(JSON.parse(body))
    if(payloads.length===2)write(res,[
      {type:'content_block_start',index:0,content_block:{type:'thinking',thinking:''}},
      {type:'content_block_delta',index:0,delta:{type:'thinking_delta',thinking:'enabled thought'}},
      {type:'content_block_delta',index:0,delta:{type:'signature_delta',signature:'enabled-signature'}},
      {type:'content_block_start',index:1,content_block:{type:'tool_use',id:'enabled_call',name:'read',input:{path:'README.md'}}},
      {type:'message_delta',delta:{stop_reason:'tool_use'}},{type:'message_stop'}
    ],false)
    else write(res,[{type:'content_block_start',index:0,content_block:{type:'text',text:'done'}},{type:'message_delta',delta:{stop_reason:'end_turn'}},{type:'message_stop'}],false)
  })})
  const model=await configure(f,url,'anthropic')
  const config=await f.request('getConfiguration')
  assert.equal(config.services[0].models[0].supportsEffort,true)
  const first=await f.run({model})
  await f.request('setEffort',{sessionId:first.id,effort:'high'})
  const second=await f.run({sessionId:first.id,model,text:'continue'})
  assert.equal(second.result.exit_reason,'completed')
  assert.equal(payloads[1].thinking.type,'enabled')
  assert.equal(payloads[1].thinking.budget_tokens,4096)
  const thinking=payloads[2].messages.find(m=>m.role==='assistant'&&m.content.some(b=>b.type==='thinking')).content[0]
  assert.deepEqual(thinking,{type:'thinking',thinking:'enabled thought',signature:'enabled-signature'})
})

test('P8: switching OpenAI history to Anthropic remaps tool ids and hands off reasoning',async t=>{
  const f=await fixture(t),anthropicPayloads=[];let openaiCount=0
  const openaiUrl=await server(t,(_req,res)=>{
    if(openaiCount++===0)write(res,[{choices:[{delta:{reasoning_content:'inspect the file'},finish_reason:null}]},{choices:[{delta:{tool_calls:[{index:0,id:'functions.read:0',function:{name:'read',arguments:'{"path":"README.md"}'}}]},finish_reason:'tool_calls'}]}])
    else write(res,[{choices:[{delta:{content:'first done'},finish_reason:'stop'}]}])
  })
  const anthropicUrl=await server(t,(req,res)=>{let body='';req.on('data',d=>body+=d);req.on('end',()=>{
    anthropicPayloads.push(JSON.parse(body))
    write(res,[{type:'content_block_start',index:0,content_block:{type:'text',text:'switched'}},{type:'message_delta',delta:{stop_reason:'end_turn'}},{type:'message_stop'}],false)
  })})
  let config=await f.request('saveService',{service:{name:'openai source',baseUrl:openaiUrl+'/v1',apiStyle:'openai',apiKey:'key',enabled:true,models:[{modelId:'deepseek-reasoner',contextWindow:128000,maxOutputTokens:8192,supportsEffort:true}]}})
  const source=config.defaultModel
  config=await f.request('saveService',{service:{name:'anthropic destination',baseUrl:anthropicUrl,apiStyle:'anthropic',apiKey:'key',enabled:true,models:[{modelId:'claude-test',contextWindow:128000,maxOutputTokens:8192,supportsEffort:false}]}})
  const destination=config.services.find(s=>s.name==='anthropic destination').id+'::claude-test'
  const first=await f.run({model:source})
  assert.equal(first.result.exit_reason,'completed')
  await f.request('setModel',{sessionId:first.id,model:destination})
  const second=await f.run({sessionId:first.id,model:destination,text:'continue'})
  assert.equal(second.result.exit_reason,'completed')
  const blocks=anthropicPayloads[0].messages.flatMap(m=>m.content)
  const call=blocks.find(b=>b.type==='tool_use'),result=blocks.find(b=>b.type==='tool_result')
  assert.match(call.id,/^[a-zA-Z0-9_-]{1,64}$/)
  assert.equal(result.tool_use_id,call.id)
  assert.ok(blocks.some(b=>b.type==='text'&&b.text.includes('inspect the file')))
  assert.equal(second.detail.messages.find(m=>m.role==='assistant').content.find(b=>b.type==='tool_use').id,'functions.read:0')
})

test('X8: discovery prefers server metadata, then catalog, then conservative defaults',async t=>{
  const f=await fixture(t),payloads=[]
  const url=await server(t,(req,res)=>{if(req.method==='POST'){let body='';req.on('data',d=>body+=d);req.on('end',()=>{payloads.push(JSON.parse(body));write(res,[{choices:[{delta:{content:'ok'},finish_reason:'stop'}]}])});return}res.setHeader('content-type','application/json');res.end(JSON.stringify({data:[
    {id:'remote-model',name:'Remote',context_window:65536,max_output_tokens:16384,input_modalities:['text','image'],effort:{supported_levels:['low','high']}},
    {id:'deepseek/deepseek-v4.1-flash'},
    {id:'unknown-model'},
    {id:'tiny-model',context_length:4096,architecture:{input_modalities:['image']}}
  ]}))})
  const service={name:'discovery',baseUrl:url+'/v1',apiStyle:'openai',apiKey:'key',models:[]}
  const discovered=await f.request('fetchServiceModels',{service})
  const [remote,known,unknown,tiny]=discovered.models
  assert.equal(remote.contextWindow,65536)
  assert.equal(remote.maxOutputTokens,16384)
  assert.equal(remote.supportsEffort,true)
  assert.equal(remote.capabilities.images,true)
  assert.equal(remote.parameterCompat.reasoningEffort.medium,'low')
  assert.equal(known.contextWindow,1000000)
  assert.equal(known.maxOutputTokens,384000)
  assert.equal(known.supportsEffort,true)
  assert.equal(unknown.contextWindow,128000)
  assert.equal(unknown.maxOutputTokens,8192)
  assert.equal(tiny.contextWindow,4096)
  assert.equal(tiny.maxOutputTokens,4095)
  assert.equal(tiny.capabilities.images,true)
  remote.contextWindow=32000;remote.maxOutputTokens=4000
  service.models=[remote];service.enabled=true
  const saved=await f.request('saveService',{service})
  assert.equal(saved.services[0].models[0].contextWindow,32000)
  assert.equal(saved.services[0].models[0].maxOutputTokens,4000)
  assert.equal(saved.services[0].models[0].parameterCompat.reasoningEffort.medium,'low')
  const first=await f.run({model:saved.defaultModel})
  assert.equal((await f.request('getState',{sessionId:first.id})).contextWindow,32000)
  assert.equal(payloads[0].max_tokens,4000)
  await f.request('setEffort',{sessionId:first.id,effort:'medium'})
  await f.run({sessionId:first.id,model:saved.defaultModel})
  assert.equal(payloads[1].reasoning_effort,'low')
})

test('X8: unconfigured unknown model uses a 128k context window',async t=>{
  const url=await server(t,(_req,res)=>write(res,[{choices:[{delta:{content:'ok'},finish_reason:'stop'}]}]))
  const f=await fixture(t,[],{COMMANDCODE_API_KEY:'key',COMMANDCODE_BASE_URL:url+'/v1'})
  const run=await f.run({model:'unknown-model'})
  const state=await f.request('getState',{sessionId:run.id})
  assert.equal(state.contextWindow,128000)
})

test('B17: connection test uses chat completions without requiring a models endpoint',async t=>{
  const f=await fixture(t),requests=[]
  const url=await server(t,(req,res)=>{let body='';req.on('data',d=>body+=d);req.on('end',()=>{
    requests.push({method:req.method,url:req.url,headers:req.headers,body:body?JSON.parse(body):null})
    if(req.url.endsWith('/models')){res.writeHead(404);res.end('no models endpoint');return}
    res.setHeader('content-type','application/json');res.end(JSON.stringify({choices:[{message:{role:'assistant',content:'ok'},finish_reason:'stop'}]}))
  })})
  const service={name:'gateway',baseUrl:url+'/v1',apiStyle:'openai',apiKey:'probe-key',models:[{modelId:'gpt-5'}]}
  const result=await f.request('testServiceConnection',{service})
  assert.ok(result.message)
  assert.equal(requests.length,1)
  assert.equal(requests[0].method,'POST')
  assert.equal(requests[0].url,'/v1/chat/completions')
  assert.equal(requests[0].headers.authorization,'Bearer probe-key')
  assert.equal(requests[0].body.model,'gpt-5')
  assert.equal(requests[0].body.stream,false)
  assert.ok(requests[0].body.max_completion_tokens>0)
  assert.equal(requests[0].body.max_tokens,undefined)
  await assert.rejects(f.request('testServiceConnection',{service:{...service,models:[{modelId:'gpt-5-pro'}]}}),/Responses API/)
  assert.equal(requests.length,1)
  await f.request('testServiceConnection',{service:{...service,models:[{modelId:'gpt-5-pro',parameterCompat:{chatSupported:true}}]}})
  assert.equal(requests.length,2)
})

test('B17: Anthropic connection probe uses messages and error bodies are bounded',async t=>{
  const f=await fixture(t),requests=[];let failure=false
  const url=await server(t,(req,res)=>{let body='';req.on('data',d=>body+=d);req.on('end',()=>{
    requests.push({url:req.url,headers:req.headers,body:body?JSON.parse(body):null})
    if(failure){res.writeHead(400);res.end('server-message: invalid model probe-key '+ 'x'.repeat(6000));return}
    if(req.url.endsWith('/models')){res.writeHead(404);res.end('missing models');return}
    res.setHeader('content-type','application/json');res.end(JSON.stringify({content:[{type:'text',text:'ok'}]}))
  })})
  const service={name:'anthropic gateway',baseUrl:url,apiStyle:'anthropic',apiKey:'probe-key',models:[{modelId:'claude-test'}]}
  await f.request('testServiceConnection',{service})
  assert.equal(requests[0].url,'/v1/messages')
  assert.equal(requests[0].headers['x-api-key'],'probe-key')
  assert.equal(requests[0].body.model,'claude-test')
  assert.ok(requests[0].body.max_tokens>0)
  failure=true
  await assert.rejects(f.request('testServiceConnection',{service}),error=>{assert.match(error.message,/server-message: invalid model/);assert.ok(!error.message.includes('probe-key'));assert.ok(error.message.length<4300);return true})
  await assert.rejects(f.request('fetchServiceModels',{service}),/server-message: invalid model/)
  await assert.rejects(f.request('testServiceConnection',{service:{...service,models:[]}}),/模型/)
})

test('A5: current catalog metadata covers Claude, OpenAI, DeepSeek and dated aliases',async t=>{
  const f=await fixture(t),payloads=[]
  const url=await server(t,(req,res)=>{if(req.method==='POST'){let body='';req.on('data',d=>body+=d);req.on('end',()=>{payloads.push(JSON.parse(body));write(res,[{choices:[{delta:{content:'ok'},finish_reason:'stop'}]}])});return}res.setHeader('content-type','application/json');res.end(JSON.stringify({data:['claude-sonnet-4-5','claude-sonnet-4-5-20250929','claude-sonnet-5-5','gpt-5.6-sol','deepseek-flash'].map(id=>({id}))}))})
  const service={name:'catalog mock',baseUrl:url+'/v1',apiStyle:'openai',apiKey:'key',models:[]}
  const discovered=await f.request('fetchServiceModels',{service})
  const models=Object.fromEntries(discovered.models.map(m=>[m.modelId,m]))
  assert.equal(models['claude-sonnet-4-5'].supportsEffort,true)
  assert.equal(models['claude-sonnet-4-5-20250929'].contextWindow,200000)
  assert.equal(models['claude-sonnet-4-5-20250929'].supportsEffort,true)
  assert.equal(models['claude-sonnet-5-5'].contextWindow,1000000)
  assert.equal(models['claude-sonnet-5-5'].maxOutputTokens,128000)
  assert.equal(models['gpt-5.6-sol'].contextWindow,1050000)
  assert.equal(models['gpt-5.6-sol'].maxOutputTokens,128000)
  assert.equal(models['gpt-5.6-sol'].parameterCompat.reasoningEffort.max,'max')
  assert.equal(models['deepseek-flash'].contextWindow,1048576)
  assert.equal(models['deepseek-flash'].maxOutputTokens,393216)
  service.models=[models['gpt-5.6-sol']];service.enabled=true
  const saved=await f.request('saveService',{service})
  const first=await f.run({model:saved.defaultModel})
  assert.equal(payloads[0].reasoning_effort,'none')
  assert.ok(payloads[0].max_completion_tokens>0)
  await f.request('setEffort',{sessionId:first.id,effort:'max'})
  const second=await f.run({sessionId:first.id,model:saved.defaultModel})
  assert.equal(second.result.exit_reason,'provider_error')
  assert.match(second.result.error,/Responses API/)
  assert.equal(payloads.length,1)
})

test('A5: built-in Claude Sonnet 4.5 can enable extended thinking',async t=>{
  const payloads=[]
  const url=await server(t,(req,res)=>{let body='';req.on('data',d=>body+=d);req.on('end',()=>{payloads.push(JSON.parse(body));write(res,[{type:'content_block_start',index:0,content_block:{type:'text',text:'ok'}},{type:'message_delta',delta:{stop_reason:'end_turn'}},{type:'message_stop'}],false)})})
  const f=await fixture(t,[],{ANTHROPIC_API_KEY:'test-key',ANTHROPIC_BASE_URL:url})
  const first=await f.run({model:'claude-sonnet-4-5'})
  await f.request('setEffort',{sessionId:first.id,effort:'high'})
  const second=await f.run({sessionId:first.id,model:'claude-sonnet-4-5'})
  assert.equal(second.result.exit_reason,'completed')
  assert.equal(payloads[1].thinking.type,'enabled')
  assert.equal(payloads[1].thinking.budget_tokens,4096)
})

test('P6: invalid tool arguments produce a recoverable failure without executing writes',async t=>{
  for(const scenario of ['stream-invalid','stream-nonobject','stream-empty','json-invalid','json-nonobject','json-empty']){
    await t.test(scenario,async t=>{
      const f=await fixture(t),payloads=[]
      const argument=scenario.endsWith('invalid')?'{"path":"unsafe",':scenario.endsWith('empty')?'':'[1,2]'
      const url=await server(t,(req,res)=>{let body='';req.on('data',d=>body+=d);req.on('end',()=>{
        payloads.push(JSON.parse(body))
        if(payloads.length>1){write(res,[{choices:[{delta:{content:'recovered'},finish_reason:'stop'}]}]);return}
        const call={id:'bad_args',function:{name:'write',arguments:argument}}
        if(scenario.startsWith('stream'))write(res,[{choices:[{delta:{tool_calls:[{index:0,...call}]},finish_reason:'tool_calls'}]}])
        else{res.setHeader('content-type','application/json');res.end(JSON.stringify({choices:[{message:{role:'assistant',content:null,tool_calls:[call]},finish_reason:'tool_calls'}]}))}
      })})
      await f.request('setPermissionMode',{mode:'bypass'})
      const result=await f.run({model:await configure(f,url)})
      assert.equal(result.result.exit_reason,'completed')
      assert.equal(fs.existsSync(path.join(f.workspace,'unsafe')),false)
      const tool=payloads[1].messages.find(m=>m.role==='tool')
      assert.equal(tool.tool_call_id,'bad_args')
      assert.match(tool.content,/参数.*(JSON|对象)/i)
      const saved=result.detail.messages.find(m=>m.role==='assistant').content.find(b=>b.type==='tool_use')
      assert.ok(saved.input_error)
      assert.ok(result.detail.events.some(e=>e.type==='tool_call_result'&&e.data.success===false))
    })
  }
})

test('P7: missing and repeated tool ids become unique across streaming rounds',async t=>{
  const f=await fixture(t),payloads=[]
  const url=await server(t,(req,res)=>{let body='';req.on('data',d=>body+=d);req.on('end',()=>{
    payloads.push(JSON.parse(body));const count=payloads.length
    const call=(index,id)=>({index,...(id===null?{}:{id}),function:{name:'read',arguments:'{"path":"README.md"}'}})
    if(count===1)write(res,[{choices:[{delta:{tool_calls:[call(0,null),call(1,'reused'),call(2,'reused'),call(3,'x'.repeat(300))]},finish_reason:'tool_calls'}]}])
    else if(count===2)write(res,[{choices:[{delta:{tool_calls:[call(0,'reused')]},finish_reason:'tool_calls'}]}])
    else write(res,[{choices:[{delta:{content:'complete'},finish_reason:'stop'}]}])
  })})
  const result=await f.run({model:await configure(f,url)})
  assert.equal(result.result.exit_reason,'completed')
  const blocks=result.detail.messages.flatMap(m=>m.content)
  const ids=blocks.filter(b=>b.type==='tool_use').map(b=>b.id)
  const resultIds=blocks.filter(b=>b.type==='tool_result').map(b=>b.tool_use_id)
  assert.equal(ids.length,5);assert.equal(new Set(ids).size,5)
  assert.ok(ids.every(id=>id&&id.length<=256))
  assert.deepEqual(resultIds,ids)
  assert.ok(payloads[2].messages.filter(m=>m.role==='tool').every(m=>ids.includes(m.tool_call_id)))
})

test('P7: non-streaming missing tool id receives a generated id',async t=>{
  const f=await fixture(t),payloads=[]
  const url=await server(t,(req,res)=>{let body='';req.on('data',d=>body+=d);req.on('end',()=>{
    payloads.push(JSON.parse(body));res.setHeader('content-type','application/json')
    res.end(JSON.stringify(payloads.length===1?{choices:[{message:{role:'assistant',content:null,tool_calls:[{function:{name:'read',arguments:'{"path":"README.md"}'}}]},finish_reason:'tool_calls'}]}:{choices:[{message:{role:'assistant',content:'ok'},finish_reason:'stop'}]}))
  })})
  const result=await f.run({model:await configure(f,url)})
  assert.equal(result.result.exit_reason,'completed')
  const assistant=payloads[1].messages.find(m=>m.role==='assistant'&&m.tool_calls)
  const tool=payloads[1].messages.find(m=>m.role==='tool')
  assert.ok(assistant.tool_calls[0].id)
  assert.equal(tool.tool_call_id,assistant.tool_calls[0].id)
})

test('P9: index-free SSE fragments remain grouped across byte boundaries',async t=>{
  for(const size of [1,2,7,31,257]){
    await t.test(`chunk-${size}`,async t=>{
      const f=await fixture(t),payloads=[]
      const delta=(tool)=>({choices:[{delta:{tool_calls:[tool]},finish_reason:null}]})
      const frames=[
        delta({id:'a',function:{name:'read',arguments:'{"path":"R'}}),
        delta({function:{arguments:'EAD'}}),
        delta({id:'b',function:{name:'read',arguments:'{"path":"READ'}}),
        delta({id:'a',function:{arguments:'ME'}}),
        delta({function:{arguments:'.md"}'}}),
        delta({id:'b',function:{arguments:'ME.md"}'}}),
        {choices:[{delta:{},finish_reason:'tool_calls'}]},
      ]
      const url=await server(t,(req,res)=>{let body='';req.on('data',d=>body+=d);req.on('end',()=>{
        payloads.push(JSON.parse(body));if(payloads.length>1){write(res,[{choices:[{delta:{content:'done'},finish_reason:'stop'}]}]);return}
        res.writeHead(200,{'content-type':'text/event-stream'});const bytes=Buffer.from(frames.map(frame=>`data: ${JSON.stringify(frame)}\n\n`).join('')+'data: [DONE]\n\n')
        for(let i=0;i<bytes.length;i+=size)res.write(bytes.subarray(i,i+size));res.end()
      })})
      const result=await f.run({model:await configure(f,url)})
      assert.equal(result.result.exit_reason,'completed')
      const calls=result.detail.messages.flatMap(m=>m.content).filter(b=>b.type==='tool_use')
      assert.deepEqual(calls.map(c=>c.id),['a','b'])
      assert.deepEqual(calls.map(c=>c.input.path),['README.md','README.md'])
      assert.equal(payloads[1].messages.filter(m=>m.role==='tool').length,2)
    })
  }
})

test('P11: empty completed responses after tools end normally without polluting future history',async t=>{
  for(const style of ['openai','anthropic']) for(const transport of ['stream','json']) for(const emptyText of [false,true]) for(const finish of (style==='anthropic'?['end_turn','stop_sequence']:['end_turn'])){
    await t.test(`${style}-${transport}-${emptyText?'empty-text':'no-content'}-${finish}`,async t=>{
      const f=await fixture(t),payloads=[]
      const url=await server(t,(req,res)=>{let body='';req.on('data',d=>body+=d);req.on('end',()=>{
        payloads.push(JSON.parse(body));const n=payloads.length
        if(transport==='json'){
          res.setHeader('content-type','application/json')
          if(style==='openai')res.end(JSON.stringify(n===1?{choices:[{message:{role:'assistant',content:null,tool_calls:[{id:'empty-test',function:{name:'read',arguments:'{"path":"README.md"}'}}]},finish_reason:'tool_calls'}],usage:{prompt_tokens:2,completion_tokens:1}}:{choices:[{message:{role:'assistant',content:n===2?(emptyText?'':null):'later'},finish_reason:'stop'}],usage:{prompt_tokens:7,completion_tokens:1}}))
          else res.end(JSON.stringify(n===1?{content:[{type:'tool_use',id:'empty-test',name:'read',input:{path:'README.md'}}],stop_reason:'tool_use',usage:{input_tokens:2,output_tokens:1}}:{content:n===2?(emptyText?[{type:'text',text:''}]:[]):[{type:'text',text:'later'}],stop_reason:finish,usage:{input_tokens:7,output_tokens:1}}))
        }else if(style==='openai'){
          write(res,n===1?[{choices:[{delta:{tool_calls:[{index:0,id:'empty-test',function:{name:'read',arguments:'{"path":"README.md"}'}}]},finish_reason:'tool_calls'}],usage:{prompt_tokens:2,completion_tokens:1}}]:[{choices:[{delta:n===2?(emptyText?{content:''}:{}):{content:'later'},finish_reason:'stop'}],usage:{prompt_tokens:7,completion_tokens:1}}])
        }else{
          write(res,n===1?[{type:'message_start',message:{usage:{input_tokens:2}}},{type:'content_block_start',index:0,content_block:{type:'tool_use',id:'empty-test',name:'read',input:{path:'README.md'}}},{type:'message_delta',delta:{stop_reason:'tool_use'},usage:{output_tokens:1}},{type:'message_stop'}]:[{type:'message_start',message:{usage:{input_tokens:7}}},...(n===2?(emptyText?[{type:'content_block_start',index:0,content_block:{type:'text',text:''}}]:[]):[{type:'content_block_start',index:0,content_block:{type:'text',text:'later'}}]),{type:'message_delta',delta:{stop_reason:finish},usage:{output_tokens:1}},{type:'message_stop'}],false)
        }
      })})
      const model=await configure(f,url,style),first=await f.run({model})
      assert.equal(first.result.exit_reason,'completed')
      assert.equal(first.result.total_usage.input_tokens,9)
      assert.ok(first.detail.events.some(e=>e.type==='assistant_message'&&e.data.stop_reason===finish))
      assert.ok(first.detail.messages.filter(m=>m.role==='assistant').every(m=>m.content.some(b=>b.type!=='text'||b.text.length>0)))
      const next=await f.run({sessionId:first.id,model,text:'continue'})
      assert.equal(next.result.exit_reason,'completed')
      assert.equal(payloads.length,3)
      assert.ok(payloads[2].messages.filter(m=>m.role==='assistant').every(m=>m.tool_calls?.length||(typeof m.content==='string'?m.content.length>0:Array.isArray(m.content)&&m.content.some(b=>b.type!=='text'||b.text.length>0))))
    })
  }
})

