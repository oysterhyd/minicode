import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import {test} from 'node:test'
import {fixture, until, pidAlive, connect} from './helpers.mjs'

async function script(f, turns) {
  fs.writeFileSync(f.script, JSON.stringify({turns}))
  await f.request('setModel', {model:'fake'})
}
async function server(t) {
  const f = await fixture(t, [
    {tool_calls:[{name:'bash',arguments:{command:'node server.cjs',background:true}}]},
    {text:'server started'},
  ])
  fs.writeFileSync(path.join(f.workspace,'server.cjs'), `
const fs=require('node:fs'),http=require('node:http');
const server=http.createServer((req,res)=>res.end('alive'));
server.listen(0,'127.0.0.1',()=>{console.log('READY');fs.writeFileSync('ready.json',JSON.stringify({pid:process.pid,port:server.address().port}))});
fs.watchFile('tick',{interval:50},()=>{if(fs.existsSync('tick'))console.log('NEW_OUTPUT')});
`)
  await f.request('setPermissionMode',{mode:'bypass'})
  const first=await f.run()
  assert.equal(first.result.exit_reason,'completed')
  const job=first.detail.events.find(e=>e.type==='background_job_started').data.job_id
  assert.equal(first.detail.events.some(e=>e.type==='background_job_lost'),false)
  assert.ok(await until(()=>fs.existsSync(path.join(f.workspace,'ready.json'))))
  const {pid,port}=JSON.parse(fs.readFileSync(path.join(f.workspace,'ready.json'),'utf8'))
  const url=`http://127.0.0.1:${port}`
  assert.equal(await (await fetch(url)).text(),'alive')
  return {f,job,pid,url}
}
const page = run => JSON.parse(run.detail.events.filter(e=>e.type==='tool_call_result'&&e.data.name==='bash_output').at(-1).data.output_detail)

test('background server survives turns, supports incremental output and repeated kill_shell',async t=>{
  const {f,job,pid,url}=await server(t)
  await script(f,[{tool_calls:[{name:'bash_output',arguments:{job_id:job}}]},{text:'read output'}])
  const first=page(await f.run({text:'read server output'}))
  assert.equal(first.status,'running');assert.match(first.output,/READY/)
  fs.writeFileSync(path.join(f.workspace,'tick'),'1')
  // A foreground wait makes the next output query observe the additional write.
  await script(f,[{tool_calls:[{name:'bash',arguments:{command:'node -e "setTimeout(()=>{},300)"'}}]},
    {tool_calls:[{name:'bash_output',arguments:{job_id:job}}]},{text:'read new output'}])
  const next=page(await f.run({text:'read added output'}))
  assert.equal(next.cursor,first.next_cursor);assert.match(next.output,/NEW_OUTPUT/);assert.doesNotMatch(next.output,/READY/)
  assert.equal(await (await fetch(url)).text(),'alive')
  await script(f,[{tool_calls:[{name:'kill_shell',arguments:{job_id:job}},{name:'kill_shell',arguments:{job_id:job}}]},{text:'stopped'}])
  const stopped=await f.run({text:'stop server'})
  assert.ok(stopped.detail.events.filter(e=>e.type==='tool_call_result'&&e.data.name==='kill_shell').every(e=>e.data.success))
  assert.ok(await until(()=>!pidAlive(pid)));await assert.rejects(fetch(url))
  assert.equal(stopped.detail.events.filter(e=>e.type==='background_job_completed').length,1)
})

test('idle cancelTurn stops background processes from earlier turns',async t=>{
  const {f,pid}=await server(t)
  await script(f,[{text:'another completed turn'}]);await f.run({text:'leave server running'})
  assert.ok(pidAlive(pid))
  assert.equal(await f.request('cancelTurn'),true)
  assert.ok(await until(()=>!pidAlive(pid)))
})

test('bridge close reaps idle background server before exiting',async t=>{
  const {f,pid}=await server(t)
  await f.close()
  assert.ok(await until(()=>!pidAlive(pid)))
  assert.equal(f.child.exitCode,0)
})

test('deleting an idle session stops its background server',async t=>{
  const {f,pid}=await server(t)
  const state=await f.request('getState')
  await f.request('deleteSession',{sessionId:state.sessionId})
  assert.ok(await until(()=>!pidAlive(pid)))
  assert.equal((await f.request('listSessions')).length,0)
})

test('idle owner cache retains a session while its background server is running',async t=>{
  const {f,pid}=await server(t)
  const original=await f.request('getState')
  for(let i=1;i<=6;i++)await f.run({clientKey:`other-${i}`,sourceClientKey:'default'})
  assert.ok(pidAlive(pid),'opening other sessions must not stop the original background task')
  assert.equal(await f.request('cancelTurn',{sessionId:original.sessionId}),true)
  assert.ok(await until(()=>!pidAlive(pid)))
})

test('a second process cannot delete a session that owns an idle background server',async t=>{
  const {f,pid}=await server(t),state=await f.request('getState'),other=connect(f)
  await assert.rejects(other.request('deleteSession',{sessionId:state.sessionId}),/session already running/)
  assert.ok(pidAlive(pid));await f.close()
  await other.request('deleteSession',{sessionId:state.sessionId})
  assert.ok(await until(()=>!pidAlive(pid)))
})
