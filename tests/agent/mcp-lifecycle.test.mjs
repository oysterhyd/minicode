import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import http from 'node:http'
import {test} from 'node:test'
import {fixture,pidAlive,until,connect} from './helpers.mjs'

const source=`const fs=require('node:fs');
fs.appendFileSync(process.argv[3]||'mcp-starts',process.argv[2]+':'+process.pid+'\\n');let counter=0;
require('node:readline').createInterface({input:process.stdin}).on('line',line=>{
const r=JSON.parse(line);if(r.id===undefined)return;
const result=r.method==='initialize'?{protocolVersion:'2025-11-25',serverInfo:{name:'stateful',version:'1'}}:
r.method==='tools/list'?{tools:[{name:'counter',inputSchema:{type:'object'}}]}:
{content:[{type:'text',text:process.argv[2]+':'+(++counter)}]};
process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:r.id,result})+'\\n')})`

async function setup(t,turns,names=['state']) {
  const f=await fixture(t,turns)
  const server=path.join(f.root,'stateful.cjs');fs.writeFileSync(server,source)
  const config={mcpServers:Object.fromEntries(names.map(name=>[name,{command:process.execPath,args:[server,name]}]))}
  fs.writeFileSync(path.join(f.workspace,'.mcp.json'),JSON.stringify(config))
  const entries=(await f.request('getLocalExtensions',{workspace:f.workspace})).entries.filter(e=>e.kind==='mcp')
  for(const entry of entries)await f.request('setLocalExtensionEnabled',{workspace:f.workspace,id:entry.id,enabled:true})
  const actual=Object.fromEntries(entries.map(e=>[e.name,e.runtimeName]))
  for(const turn of turns)for(const tool of turn.tool_calls||[]) {
    const match=/^mcp__(\w+)__counter$/.exec(tool.name)
    if(match)tool.name=`mcp__${actual[match[1]]}__counter`
  }
  fs.writeFileSync(f.script,JSON.stringify({turns}))
  await f.request('setPermissionMode',{mode:'bypass'})
  return {f,config,starts:()=>fs.readFileSync(path.join(f.workspace,'mcp-starts'),'utf8').trim().split('\n')}
}
const call=name=>({tool_calls:[{name:`mcp__${name}__counter`,arguments:{}}]})
const results=r=>r.detail.events.filter(e=>e.type==='tool_call_result'&&e.data.name.startsWith('mcp__')).map(e=>e.data.output_detail)

test('R6 five turns reuse one stateful MCP process and close reaps it',async t=>{
  const {f,starts}=await setup(t,Array.from({length:5},()=>[call('state'),{text:'done'}]).flat())
  for(let i=1;i<=5;i++)assert.equal(results(await f.run({text:`round ${i}`})).at(-1),`state:${i}`)
  assert.equal(starts().length,1)
  const pid=Number(starts()[0].split(':')[1]);assert.ok(pidAlive(pid));await f.close()
  assert.ok(await until(()=>!pidAlive(pid)))
})

test('R6 changing one imported server reconnects only that server',async t=>{
  const turns=[call('a'),call('b'),{text:'first'},call('a'),call('b'),{text:'second'}]
  const {f,config,starts}=await setup(t,turns,['a','b'])
  assert.deepEqual(results(await f.run()),['a:1','b:1'])
  config.mcpServers.a.args[1]='changed'
  fs.writeFileSync(path.join(f.workspace,'.mcp.json'),JSON.stringify(config))
  assert.deepEqual(results(await f.run({text:'again'})).slice(-2),['changed:1','b:2'])
  assert.equal(starts().length,3)
})

test('R6 a child without MCP tools never starts another server',async t=>{
  const {f,starts}=await setup(t,[
    {tool_calls:[{name:'delegate',arguments:{kind:'explore',task:'read README'}}]},
    {tool_calls:[{name:'read',arguments:{path:'README.md'}}]},
    {text:JSON.stringify({summary:'read',findings:[],evidence_refs:['README.md'],unresolved:[]})},
    {text:'parent done'},
  ])
  const run=await f.run();assert.equal(run.result.exit_reason,'completed');assert.equal(starts().length,1)
})

test('R35 child borrows whitelisted parent MCP connection and child close preserves its state',async t=>{
  const turns=[call('state'),{tool_calls:[{name:'delegate',arguments:{kind:'counter-agent',task:'count'}}]},call('state'),{text:JSON.stringify({summary:'counted',findings:[],evidence_refs:[],unresolved:[]})},call('state'),{text:'parent done'},call('state'),{text:'next turn'}]
  const {f,starts}=await setup(t,turns)
  await f.request('saveAgent',{agent:{name:'counter-agent',description:'counter',instructions:'count',tools:[turns[0].tool_calls[0].name],inheritTools:false}})
  const run=await f.run()
  const childId=run.detail.events.find(e=>e.type==='subagent_start').data.child_session_id
  const child=await f.request('getSession',{sessionId:childId})
  assert.deepEqual(results({detail:child}),['state:2'])
  assert.deepEqual(results(run),['state:1','state:3'])
  assert.equal(starts().length,1)
  assert.equal(results(await f.run({text:'next'})).at(-1),'state:4')
  assert.equal(starts().length,1)
})

test('R35 child failure cannot close borrowed parent MCP connections',async t=>{
  const turns=[call('state'),{tool_calls:[{name:'delegate',arguments:{kind:'counter-agent',task:'fail'}}]},{error:'provider simulated failure'},call('state'),{text:'handled'},call('state'),{text:'next turn'}]
  const {f,starts}=await setup(t,turns)
  await f.request('saveAgent',{agent:{name:'counter-agent',description:'counter',instructions:'count',tools:[],inheritTools:true}})
  assert.deepEqual(results(await f.run()),['state:1','state:2'])
  assert.equal(results(await f.run({text:'next'})).at(-1),'state:3')
  assert.equal(starts().length,1)
})

test('R35 sharing a registry never grants sibling MCP tools to a child',async t=>{
  const turns=[{tool_calls:[{name:'delegate',arguments:{kind:'limited',task:'attempt forbidden server'}}]},call('b'),{text:JSON.stringify({summary:'forbidden tool refused',findings:[],evidence_refs:[],unresolved:[]})},call('b'),{text:'parent done'}]
  const {f,starts}=await setup(t,turns,['a','b'])
  const entries=(await f.request('getLocalExtensions',{workspace:f.workspace})).entries
  const allowed=entries.find(e=>e.kind==='mcp'&&e.name==='a').runtimeName
  await f.request('saveAgent',{agent:{name:'limited',description:'only a',instructions:'use a',tools:[`mcp__${allowed}__counter`],inheritTools:false}})
  for(const name of ['mcp__a__*','mcp____counter','mcp__a__']) await assert.rejects(f.request('saveAgent',{agent:{name:'invalid',description:'invalid',instructions:'test',tools:[name],inheritTools:false}}),/未知工具/)
  const run=await f.run()
  const child=await f.request('getSession',{sessionId:run.detail.events.find(e=>e.type==='subagent_start').data.child_session_id})
  assert.ok(!child.events.some(e=>e.type==='tool_call_result'&&e.data.name===turns[1].tool_calls[0].name&&e.data.success))
  assert.deepEqual(results(run),['b:1']);assert.equal(starts().length,2)
})

test('R6 disconnected MCP reconnects on the next turn',async t=>{
  const {f,starts}=await setup(t,[call('state'),{text:'first'},call('state'),{text:'second'}])
  assert.equal(results(await f.run()).at(-1),'state:1')
  const pid=Number(starts()[0].split(':')[1]);process.kill(pid)
  assert.ok(await until(()=>!pidAlive(pid)))
  assert.equal(results(await f.run({text:'recover server'})).at(-1),'state:1')
  assert.equal(starts().length,2)
})

test('R6 a changed trusted plugin reconnects only its own server',async t=>{
  const f=await fixture(t,[call('a'),call('b'),{text:'first'},call('a'),call('b'),{text:'second'}])
  const server=path.join(f.root,'stateful.cjs');fs.writeFileSync(server,source)
  const version=JSON.parse(fs.readFileSync(new URL('../../release/package.json',import.meta.url),'utf8')).version
  const log=path.join(f.workspace,'mcp-starts')
  for(const name of ['a','b']) {
    const root=path.join(f.workspace,'.minicode/plugins',name);fs.mkdirSync(root,{recursive:true})
    fs.writeFileSync(path.join(root,'plugin.json'),JSON.stringify({name,version:'1.0.0',minicode_version:version,enabled:true,mcp_servers:[{name,command:process.execPath,args:[server,name,log]}]}))
  }
  await f.request('trustWorkspace',{workspace:f.workspace})
  await f.request('setPermissionMode',{mode:'bypass'})
  assert.deepEqual(results(await f.run()),['a:1','b:1'])
  fs.writeFileSync(path.join(f.workspace,'.minicode/plugins/a/extra.txt'),'updated')
  await f.request('lockPlugins',{workspace:f.workspace})
  await f.request('trustWorkspace',{workspace:f.workspace})
  assert.deepEqual(results(await f.run({text:'again'})).slice(-2),['a:1','b:2'])
  assert.equal(fs.readFileSync(log,'utf8').trim().split('\n').length,3)
})

test('R6 HTTP sessions persist across turns and expired sessions reconnect within a turn',async t=>{
  const f=await fixture(t);let initialized=0,expire=false
  const counts=new Map()
  const server=http.createServer((req,res)=>{
    let body='';req.on('data',b=>body+=b);req.on('end',()=>{
      const r=JSON.parse(body);res.setHeader('content-type','application/json')
      if(r.id===undefined){res.writeHead(202);res.end();return}
      let result
      if(r.method==='initialize') {
        const session=String(++initialized);counts.set(session,0);res.setHeader('Mcp-Session-Id',session)
        result={protocolVersion:'2025-11-25',serverInfo:{name:'stateful',version:'1'}}
      } else if(r.method==='tools/list')result={tools:[{name:'counter',inputSchema:{type:'object'}}]}
      else {
        const session=req.headers['mcp-session-id']
        if(expire){expire=false;counts.delete(session);res.writeHead(404);res.end('{}');return}
        const count=counts.get(session)+1;counts.set(session,count)
        result={content:[{type:'text',text:`${session}:${count}`} ]}
      }
      res.end(JSON.stringify({jsonrpc:'2.0',id:r.id,result}))
    })
  })
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));t.after(()=>new Promise(resolve=>server.close(resolve)))
  fs.writeFileSync(path.join(f.workspace,'.mcp.json'),JSON.stringify({mcpServers:{web:{url:`http://127.0.0.1:${server.address().port}`,transport:'http'}}}))
  const entry=(await f.request('getLocalExtensions',{workspace:f.workspace})).entries.find(e=>e.kind==='mcp')
  await f.request('setLocalExtensionEnabled',{workspace:f.workspace,id:entry.id,enabled:true})
  const tool={tool_calls:[{name:`mcp__${entry.runtimeName}__counter`,arguments:{}}]}
  fs.writeFileSync(f.script,JSON.stringify({turns:[tool,{text:'first'},tool,{text:'second'},tool,tool,{text:'recovered'}]}))
  await f.request('setPermissionMode',{mode:'bypass'})
  assert.equal(results(await f.run()).at(-1),'1:1')
  assert.equal(results(await f.run({text:'again'})).at(-1),'1:2');assert.equal(initialized,1)
  expire=true
  const recovered=await f.run({text:'recover'})
  assert.equal(results(recovered).at(-1),'2:1');assert.equal(initialized,2)
  assert.equal(recovered.result.exit_reason,'completed')
})

test('R6 idle owner cache retains sessions with live MCP connections',async t=>{
  const {f,starts}=await setup(t,[call('state'),{text:'first'},call('state'),{text:'second'}])
  const first=await f.run({clientKey:'view-0',sourceClientKey:'default'})
  for(let i=1;i<=6;i++)assert.equal(results(await f.run({clientKey:`view-${i}`,sourceClientKey:'default'})).at(-1),'state:1')
  const continued=await f.run({clientKey:'view-0',sessionId:first.id,text:'continue same connection'})
  assert.equal(results(continued).at(-1),'state:2')
  assert.equal(starts().length,7)
  const pids=starts().map(line=>Number(line.split(':')[1]));await f.close()
  assert.ok(await until(()=>pids.every(pid=>!pidAlive(pid))))
})

test('R6 a second process cannot delete an idle session that owns an MCP connection',async t=>{
  const {f,starts}=await setup(t,[call('state'),{text:'done'}]);const run=await f.run(),other=connect(f)
  await assert.rejects(other.request('deleteSession',{sessionId:run.id}),/session already running/)
  const pid=Number(starts()[0].split(':')[1]);assert.ok(pidAlive(pid));await f.close()
  await other.request('deleteSession',{sessionId:run.id});assert.ok(await until(()=>!pidAlive(pid)))
})
