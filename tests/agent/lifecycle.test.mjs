import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import {test} from 'node:test'
import {DatabaseSync} from 'node:sqlite'
import {fixture,pidAlive,until} from './helpers.mjs'
// R41 / B16: a run that panics or fails while tearing down still reports
// run_error, clears "running" and stops its background jobs and MCP servers.
async function withProcesses(f,last){
  const jobPid=path.join(f.root,'job.pid'),mcpPid=path.join(f.root,'mcp.pid'),server=path.join(f.root,'server.cjs')
  fs.writeFileSync(server,`require('node:fs').writeFileSync(${JSON.stringify(mcpPid)},String(process.pid));const rl=require('node:readline').createInterface({input:process.stdin});rl.on('line',line=>{const r=JSON.parse(line);if(r.id===undefined)return;const result=r.method==='initialize'?{protocolVersion:'2025-11-25',capabilities:{tools:{}},serverInfo:{name:'fixture',version:'1'}}:r.method==='tools/list'?{tools:[]}:{};process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:r.id,result})+'\\n')})`)
  fs.writeFileSync(path.join(f.workspace,'.mcp.json'),JSON.stringify({mcpServers:{fixture:{command:process.execPath,args:[server]}}}))
  const entry=(await f.request('getLocalExtensions',{workspace:f.workspace})).entries.find(e=>e.kind==='mcp');await f.request('setLocalExtensionEnabled',{workspace:f.workspace,id:entry.id,enabled:true})
  const job=`node -e "require('fs').writeFileSync('${jobPid.replaceAll('\\','/')}',String(process.pid));setTimeout(()=>{},60000)"`
  fs.writeFileSync(f.script,JSON.stringify({turns:[{tool_calls:[{name:'bash',arguments:{command:job,background:true}},{name:'bash',arguments:{command:'node -e "setTimeout(()=>{},1000)"'}}]},last,{text:'recovered'}]}))
  await f.request('setPermissionMode',{mode:'bypass'})
  return {pids:()=>[jobPid,mcpPid].map(p=>Number(fs.readFileSync(p,'utf8')))}
}
async function assertStopped(f,processes,id){
  assert.equal((await f.request('getState',{sessionId:id})).running,false)
  for(const pid of processes.pids())assert.ok(await until(()=>!pidAlive(pid)),`process ${pid} still running`)
}
test('a panic during a run reports run_error, resets running and stops its processes',async t=>{
  const f=await fixture(t);const processes=await withProcesses(f,{panic:'boom'})
  const r=await f.settle();assert.equal(r.done.event,'run_error');assert.match(r.done.error,/panic.*boom/)
  await assertStopped(f,processes,r.id)
  const detail=await f.request('getSession',{sessionId:r.id});assert.equal(detail.events.find(e=>e.type==='session_end').data.exit_reason,'internal_error')
  const next=await f.run({sessionId:r.id,text:'again'});assert.equal(next.result.exit_reason,'completed')
})
test('a storage failure while tearing down reports run_error and still stops its processes',async t=>{
  const f=await fixture(t);const processes=await withProcesses(f,{text:'done'})
  const db=new DatabaseSync(f.db);db.exec(`CREATE TRIGGER inject_failure BEFORE INSERT ON events WHEN NEW.type='session_end' BEGIN SELECT RAISE(ABORT,'injected storage failure'); END;`)
  const r=await f.settle();assert.equal(r.done.event,'run_error');assert.match(r.done.error,/injected storage failure/)
  await assertStopped(f,processes,r.id)
  db.exec('DROP TRIGGER inject_failure');db.close()
})

// B-COMPAT: the handshake states the protocol so clients can give an upgrade
// hint instead of failing on the first unknown event.
test('initialize reports the protocol version, the minimum client and capabilities',async t=>{
  const f=await fixture(t);const initial=await f.request('initialize',{workspace:f.workspace})
  assert.equal(initial.protocolVersion,2);assert.equal(initial.minClientProtocol,2);assert.equal(initial.state.protocolVersion,2)
  assert.ok(initial.capabilities.includes('workspaceTrust'))
})
