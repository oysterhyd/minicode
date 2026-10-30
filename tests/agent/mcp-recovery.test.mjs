import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import {test} from 'node:test'
import {allowMockMcp,fixture,pidAlive,until} from './helpers.mjs'

async function setup(t,code,turns,limited) {
  const f=await fixture(t),file=path.join(f.root,'server.cjs')
  fs.writeFileSync(file,code)
  fs.writeFileSync(path.join(f.workspace,'.mcp.json'),JSON.stringify({mcpServers:{recover:{command:process.execPath,args:[file],timeout_s:5}}}))
  const entry=(await f.request('getLocalExtensions',{workspace:f.workspace})).entries.find(e=>e.kind==='mcp')
  await f.request('setLocalExtensionEnabled',{workspace:f.workspace,id:entry.id,enabled:true})
  const name=tool=>`mcp__${entry.runtimeName}__${tool}`
  const configPath=path.join(f.root,'.minicode/desktop-config.json'),config=JSON.parse(fs.readFileSync(configPath,'utf8'))
  config.toolTimeouts={[name(limited)]:0.1};fs.writeFileSync(configPath,JSON.stringify(config))
  fs.writeFileSync(f.script,JSON.stringify({turns:turns(name)}))
  allowMockMcp(f);await f.request('setPermissionMode',{mode:'bypass'})
  return f
}
const call=(name,arguments_={})=>({tool_calls:[{name,arguments:arguments_}]})
const boot=`const fs=require('node:fs');fs.appendFileSync('mcp-starts',process.pid+'\\n');
const send=(id,result)=>console.log(JSON.stringify({jsonrpc:'2.0',id,result}));
const rl=require('node:readline').createInterface({input:process.stdin});setInterval(()=>{},1000);
`
const initialize=`if(r.id===undefined)return;
if(r.method==='initialize')return send(r.id,{protocolVersion:'2025-11-25',serverInfo:{name:'recovery',version:'1'}});
`

test('MC4 a timed-out partial request write discards the connection before the next call',async t=>{
  const source=boot+`const first=fs.readFileSync('mcp-starts','utf8').trim().split('\\n').length===1;
rl.on('line',line=>{const r=JSON.parse(line);${initialize}
if(r.method==='tools/list'){send(r.id,{tools:['large','fast'].map(name=>({name,inputSchema:{type:'object'}}))});if(first)rl.pause();return;}
send(r.id,{content:[{type:'text',text:'fast-ok'}]});});`
  const f=await setup(t,source,name=>[call(name('large'),{value:'x'.repeat(1024*1024)}),call(name('fast')),{text:'recovered'}],'large')
  const result=await f.run(),tools=result.detail.events.filter(e=>e.type==='tool_call_result')
  assert.equal(tools[0].data.success,false);assert.match(tools[0].data.error,/timed out/)
  assert.equal(tools[1].data.success,true,tools[1].data.error)
  assert.equal(tools[1].data.output_detail,'fast-ok')
  const starts=fs.readFileSync(path.join(f.workspace,'mcp-starts'),'utf8').trim().split('\n')
  assert.equal(starts.length,2);assert.ok(await until(()=>!pidAlive(Number(starts[0]))))
})

test('MC4 a late half-frame response is drained and cannot replace the next result',async t=>{
  const source=boot+`let partial=false;const queued=[];
rl.on('line',line=>{const r=JSON.parse(line);${initialize}
if(r.method==='tools/list')return send(r.id,{tools:['slow','fast'].map(name=>({name,inputSchema:{type:'object'}}))});
if(r.params.name==='slow'){
partial=true;const frame=JSON.stringify({jsonrpc:'2.0',id:r.id,result:{content:[{type:'text',text:'late-result'}]}})+'\\n';
const half=Math.floor(frame.length/2);process.stdout.write(frame.slice(0,half));
setTimeout(()=>{process.stdout.write(frame.slice(half));partial=false;for(const id of queued)send(id,{content:[{type:'text',text:'correct-next-result'}]});},1000);
}else if(partial)queued.push(r.id);else send(r.id,{content:[{type:'text',text:'correct-next-result'}]});});`
  const f=await setup(t,source,name=>[call(name('slow')),call(name('fast')),{text:'recovered'}],'slow')
  const result=await f.run(),tools=result.detail.events.filter(e=>e.type==='tool_call_result')
  assert.equal(tools[0].data.success,false);assert.match(tools[0].data.error,/timed out/)
  assert.equal(tools[1].data.output_detail,'correct-next-result');assert.equal(tools[1].data.success,true)
  assert.equal(fs.readFileSync(path.join(f.workspace,'mcp-starts'),'utf8').trim().split('\n').length,1)
})

test('MC4 an invalid response frame closes the stream and reconnects',async t=>{
  const source=boot+`const first=fs.readFileSync('mcp-starts','utf8').trim().split('\\n').length===1;
rl.on('line',line=>{const r=JSON.parse(line);${initialize}
if(r.method==='tools/list')return send(r.id,{tools:['bad','fast'].map(name=>({name,inputSchema:{type:'object'}}))});
if(first)console.log('invalid frame');else send(r.id,{content:[{type:'text',text:'after-reconnect'}]});});`
  const f=await setup(t,source,name=>[call(name('bad')),call(name('fast')),{text:'recovered'}],'bad')
  const result=await f.run(),tools=result.detail.events.filter(e=>e.type==='tool_call_result')
  assert.equal(tools[0].data.success,false);assert.match(tools[0].data.error,/frame/)
  assert.equal(tools[1].data.success,true);assert.equal(tools[1].data.output_detail,'after-reconnect')
  assert.equal(fs.readFileSync(path.join(f.workspace,'mcp-starts'),'utf8').trim().split('\n').length,2)
})
