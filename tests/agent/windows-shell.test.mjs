import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import {test} from 'node:test'
import {fixture} from './helpers.mjs'

const hasGitBash=process.platform==='win32' && (process.env.PATH || '').split(';').some(dir=>
  fs.existsSync(path.join(dir,'git.exe')) && ['../bin/bash.exe','../usr/bin/bash.exe'].some(candidate=>fs.existsSync(path.resolve(dir,candidate))))

test('Windows bash uses Git Bash and preserves its exit status', {skip:!hasGitBash}, async t=>{
  const f=await fixture(t,[{tool_calls:[{name:'bash',arguments:{command:"printf 'git-bash-ok\\n' && exit 7"}}]},{text:'done'}])
  await f.request('setPermissionMode',{mode:'bypass'})
  const result=await f.run()
  const tool=result.detail.events.find(e=>e.type==='tool_call_result')
  assert.equal(tool.data.exit_code,7)
  assert.match(tool.data.output_detail,/git-bash-ok/)
})

test('Windows PowerShell tool preserves native exit codes and supports its legacy alias', {skip:process.platform!=='win32'}, async t=>{
  const f=await fixture(t,[{tool_calls:[
    {name:'powershell',arguments:{command:'node -e "process.exit(7)"'}},
    {name:'bash',arguments:{command:'node -e "process.exit(9)"'}},
  ]},{text:'done'}],{MINICODE_WINDOWS_SHELL:'powershell'})
  await f.request('setPermissionMode',{mode:'bypass'})
  const result=await f.run()
  const tools=result.detail.events.filter(e=>e.type==='tool_call_result')
  assert.equal(tools[0].data.exit_code,7)
  assert.equal(tools[1].data.exit_code,9)
})

test('Windows falls back to PowerShell when Git Bash is unavailable', {skip:process.platform!=='win32'}, async t=>{
  const systemRoot=process.env.SystemRoot || 'C:\\Windows'
  const cleanPath=[path.dirname(process.execPath),path.join(systemRoot,'System32'),path.join(systemRoot,'System32','WindowsPowerShell','v1.0')].join(';')
  const f=await fixture(t,[{tool_calls:[{name:'bash',arguments:{command:'node -e "process.exit(11)"'}}]},{text:'done'}],{PATH:cleanPath})
  await f.request('setPermissionMode',{mode:'bypass'})
  const result=await f.run()
  assert.equal(result.detail.events.find(e=>e.type==='tool_call_result').data.exit_code,11)
})

test('PowerShell background jobs follow command approval and scoped instructions', {skip:process.platform!=='win32'}, async t=>{
  const f=await fixture(t,[{tool_calls:[{name:'powershell',arguments:{command:'node -e "process.exit(0)"',background:true,cwd:'sub'}}]},{text:'done'}])
  fs.mkdirSync(path.join(f.workspace,'sub'))
  fs.writeFileSync(path.join(f.workspace,'sub','AGENTS.md'),'Nested rules')
  await f.request('setPermissionMode',{mode:'bypass'})
  const result=await f.run()
  const tool=result.detail.events.find(e=>e.type==='tool_call_result')
  assert.match(tool.data.error,/AGENTS\.md/)
  assert.equal(result.detail.events.some(e=>e.type==='background_job_started'),false)

  const f2=await fixture(t,[{tool_calls:[
    {name:'powershell',arguments:{command:'node -e "setTimeout(()=>console.log(42),200)"',background:true}},
    {name:'powershell',arguments:{command:'node -e "setTimeout(()=>{},800)"'}},
  ]},{text:'done'}])
  await f2.request('setPermissionMode',{mode:'bypass'})
  const next=await f2.run({text:'start background'})
  assert.equal(next.detail.events.filter(e=>e.type==='background_job_started').length,1)
  assert.equal(next.detail.events.filter(e=>['background_job_completed','background_job_lost'].includes(e.type)).length,1)
})
