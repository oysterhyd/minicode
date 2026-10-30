import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import http from 'node:http'
import {test} from 'node:test'
import {fixture} from './helpers.mjs'

function rules(f,value,file='settings.json') {fs.mkdirSync(path.join(f.workspace,'.minicode'),{recursive:true});fs.writeFileSync(path.join(f.workspace,'.minicode',file),JSON.stringify({permissions:value}))}
const call=(name,arguments_)=>({tool_calls:[{name,arguments:arguments_}]})

test('R26 layered deny beats a local allow and filters grep content and path aliases',async t=>{
  const f=await fixture(t,[call('read',{path:'sub/../.env'}),call('grep',{pattern:'PRIVATE_KEY',include_ignored:true}),{text:'done'}])
  fs.mkdirSync(path.join(f.workspace,'sub'));fs.writeFileSync(path.join(f.workspace,'.env'),'PRIVATE_KEY=secret')
  rules(f,{deny:['read(.env)']});rules(f,{allow:['read(*)','grep']},'settings.local.json')
  await f.request('setPermissionMode',{mode:'bypass'});const run=await f.run()
  const reads=run.detail.events.filter(e=>e.type==='tool_call_result')
  assert.equal(reads[0].data.success,false);assert.ok(!reads[1].data.output_detail.includes('PRIVATE_KEY=secret'))
})

test('R26 session bash approval grants only the displayed prefix',async t=>{
  const f=await fixture(t,[call('bash',{command:'node --version'}),call('bash',{command:'node -e "console.log(123)"'}),{text:'done'}])
  const started=await f.request('sendPrompt',{workspace:f.workspace,model:'fake',text:'check'})
  const first=await f.wait(e=>e.event==='approval');assert.ok(first.request.grant_rule)
  await f.request('resolveApproval',{sessionId:started.sessionId,approvalId:first.approvalId,granted:true,remember:'session'})
  const from=f.events.length
  const second=await f.wait(e=>e.event==='approval',from)
  assert.notEqual(second.approvalId,first.approvalId)
  await f.request('resolveApproval',{sessionId:started.sessionId,approvalId:second.approvalId,granted:false})
  await f.wait(e=>e.event==='run_done')
})

test('R26 approved command prefixes cannot authorize a denied compound segment',async t=>{
  const f=await fixture(t,[call('bash',{command:'node --version; rm -rf must-stay'}),{text:'done'}])
  rules(f,{allow:['bash(node --version:*)'],deny:['bash(rm -rf:*)']})
  fs.mkdirSync(path.join(f.workspace,'must-stay'));await f.request('setPermissionMode',{mode:'bypass'})
  const run=await f.run();assert.equal(run.detail.events.find(e=>e.type==='tool_call_result').data.success,false)
  assert.ok(fs.existsSync(path.join(f.workspace,'must-stay')))
  assert.ok(!run.events.some(e=>e.event==='approval'))
})

test('R26 project approval stores only the reviewed rule and later reuses it',async t=>{
  const f=await fixture(t,[call('write',{path:'one.txt',content:'one'}),{text:'first'},call('write',{path:'two.txt',content:'two'}),{text:'second'}])
  const started=await f.request('sendPrompt',{workspace:f.workspace,model:'fake',text:'write'})
  const approval=await f.wait(e=>e.event==='approval')
  await f.request('resolveApproval',{sessionId:started.sessionId,approvalId:approval.approvalId,granted:true,remember:'project'})
  await f.wait(e=>e.event==='run_done')
  const saved=JSON.parse(fs.readFileSync(path.join(f.workspace,'.minicode/settings.local.json'),'utf8'))
  assert.deepEqual(saved.permissions.allow,['write(one.txt)'])
  const from=f.events.length;await f.request('sendPrompt',{workspace:f.workspace,sessionId:started.sessionId,model:'fake',text:'different file'})
  const next=await f.wait(e=>e.event==='approval',from)
  await f.request('resolveApproval',{sessionId:started.sessionId,approvalId:next.approvalId,granted:false});await f.wait(e=>e.event==='run_done',from)
  assert.ok(!fs.existsSync(path.join(f.workspace,'two.txt')))
})

test('R26 bypass still asks for MCP and an explicit scoped approval can be remembered',async t=>{
  const f=await fixture(t)
  const server=http.createServer((req,res)=>{let body='';req.on('data',chunk=>body+=chunk);req.on('end',()=>{const r=JSON.parse(body);res.setHeader('content-type','application/json');if(r.id===undefined){res.writeHead(202);res.end();return}const result=r.method==='initialize'?{protocolVersion:'2025-11-25'}:r.method==='tools/list'?{tools:[{name:'echo',inputSchema:{type:'object'}}]}:{content:[{type:'text',text:'authorized'}]};res.end(JSON.stringify({jsonrpc:'2.0',id:r.id,result}))})})
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));t.after(()=>new Promise(resolve=>server.close(resolve)))
  fs.writeFileSync(path.join(f.workspace,'.mcp.json'),JSON.stringify({mcpServers:{fixture:{transport:'http',url:`http://127.0.0.1:${server.address().port}`}}}))
  const entry=(await f.request('getLocalExtensions',{workspace:f.workspace})).entries.find(e=>e.kind==='mcp')
  await f.request('setLocalExtensionEnabled',{workspace:f.workspace,id:entry.id,enabled:true})
  const name=`mcp__${entry.runtimeName}__echo`
  fs.writeFileSync(f.script,JSON.stringify({turns:[call(name,{}),call(name,{}),{text:'done'}]}))
  await f.request('setPermissionMode',{mode:'bypass'})
  const started=await f.request('sendPrompt',{workspace:f.workspace,model:'fake',text:'call mock'})
  const approval=await f.wait(e=>e.event==='approval');assert.equal(approval.request.tool_name,name);assert.equal(approval.request.grant_rule,name)
  await f.request('resolveApproval',{sessionId:started.sessionId,approvalId:approval.approvalId,granted:true,remember:'session'})
  await f.wait(e=>e.event==='run_done')
  assert.equal(f.events.filter(e=>e.event==='approval').length,1)
})
