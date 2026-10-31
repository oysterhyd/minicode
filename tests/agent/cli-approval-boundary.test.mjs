import assert from 'node:assert/strict'
import test from 'node:test'
import {spawn,spawnSync} from 'node:child_process'
import fs from 'node:fs'
import path from 'node:path'
import {fixture,executable} from './helpers.mjs'
function env(f){return {...process.env,HOME:f.root,USERPROFILE:f.root,MINICODE_DB_PATH:f.db,MINICODE_FAKE_SCRIPT:f.script}}
test('L12 pretyped y cannot approve a later request',async t=>{
 const f=await fixture(t,[{tool_calls:[{name:'write',arguments:{path:'unsafe.txt',content:'unsafe'}}]},{text:'done'}])
 await f.request('listSessions')
 const run=spawnSync(executable,['run','inspect','--script',f.script,'--workspace',f.workspace],{env:env(f),input:'y\n',encoding:'utf8',timeout:12000})
 assert.equal(run.status,0,run.stderr)
 assert.equal(fs.existsSync(path.join(f.workspace,'unsafe.txt')),false)
})
test('L12 a fresh request confirmation approves exactly that request',async t=>{
 const f=await fixture(t,[{tool_calls:[{name:'write',arguments:{path:'confirmed.txt',content:'ok'}}]},{text:'done'}])
 await f.request('listSessions')
 const child=spawn(executable,['run','inspect','--script',f.script,'--workspace',f.workspace],{env:env(f),windowsHide:true,stdio:['pipe','pipe','pipe']})
 let output='',stderr='',sent=false
 child.stdout.on('data',chunk=>{output+=chunk;const code=/确认码：([a-f0-9]{8})/.exec(output)?.[1];if(code&&!sent){sent=true;child.stdin.write(`pending user message\n/follow after current\n/steer change direction\ny ${code}\n`)}})
 child.stderr.on('data',chunk=>stderr+=chunk)
 const timeout=setTimeout(()=>child.kill(),12000)
 const code=await new Promise(resolve=>child.on('exit',resolve));clearTimeout(timeout)
 assert.equal(code,0,stderr+output);assert.equal(sent,true)
 assert.equal(fs.readFileSync(path.join(f.workspace,'confirmed.txt'),'utf8'),'ok')
 const [session]=await f.request('listSessions'),detail=await f.request('getSession',{sessionId:session.session_id})
 for(const text of ['change direction','after current']) assert.equal(detail.messages.flatMap(m=>m.content).filter(b=>b.text===text).length,1)
 await f.request('selectSession',{sessionId:session.session_id,workspace:f.workspace,deferRuntime:true})
 assert.ok((await f.request('listQueue',{sessionId:session.session_id})).some(item=>item.text==='pending user message'))
})
