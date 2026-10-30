import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import {test} from 'node:test'
import {DatabaseSync} from 'node:sqlite'
import {fixture} from './helpers.mjs'

const final=refs=>({text:JSON.stringify({summary:'observed',findings:[],evidence_refs:refs,unresolved:[]})})
const delegate=kind=>({tool_calls:[{name:'delegate',arguments:{kind,task:'report evidence'}}]})
const payload=run=>JSON.parse(run.detail.events.filter(e=>e.type==='tool_call_result'&&e.data.name==='delegate').at(-1).data.output_detail)

test('R31b grep glob hits without path verify normalized references and reject fabricated files',async t=>{
  const refs=['./README.md:1','README.md:1:2',...(process.platform==='win32'?['.\\README.md:1','readme.MD:1']:[]),'untouched.md:1','../outside.md:1']
  const f=await fixture(t,[delegate('explore'),{tool_calls:[{name:'grep',arguments:{pattern:'repository',glob:'*.md'}}]},final(refs),{text:'parent done'}])
  fs.writeFileSync(path.join(f.workspace,'README.md'),'repository untouched.md:1: forged reference\n')
  fs.writeFileSync(path.join(f.workspace,'untouched.md'),'not a match\n');fs.writeFileSync(path.join(f.root,'outside.md'),'repository outside\n')
  const result=payload(await f.run())
  assert.deepEqual(result.evidence_refs,refs.slice(0,-2))
  assert.ok(result.unresolved.some(s=>s.includes('untouched.md')));assert.ok(result.unresolved.some(s=>s.includes('../outside.md')))
})

test('R31b equivalent explicit read paths refer to the same observed file',async t=>{
  const refs=['README.md:1','./README.md:1']
  const f=await fixture(t,[delegate('explore'),{tool_calls:[{name:'read',arguments:{path:'./README.md'}}]},final(refs),{text:'parent done'}])
  assert.deepEqual(payload(await f.run()).evidence_refs,refs)
})

test('R31b listing evidence includes only the files actually shown on the page',async t=>{
  const f=await fixture(t,[delegate('explore'),{tool_calls:[{name:'ls',arguments:{limit:1}}]},final(['README.md','z.md']),{text:'parent done'}])
  fs.writeFileSync(path.join(f.workspace,'z.md'),'not on this page')
  assert.deepEqual(payload(await f.run()).evidence_refs,['README.md'])
})

test('R31b unused path parameters on a non-file tool cannot forge evidence',async t=>{
  const f=await fixture(t,[delegate('meta'),{tool_calls:[{name:'skills_list',arguments:{path:'unread.md'}}]},final(['unread.md']),{text:'parent done'}])
  fs.writeFileSync(path.join(f.workspace,'unread.md'),'never read')
  await f.request('saveAgent',{agent:{name:'meta',description:'metadata only',instructions:'list skills',tools:[],inheritTools:true}})
  const result=payload(await f.run())
  assert.deepEqual(result.evidence_refs,[]);assert.ok(result.unresolved.some(s=>s.includes('unread.md')))
})

test('R31b an explicit grep path without a match is not file evidence',async t=>{
  const f=await fixture(t,[delegate('explore'),{tool_calls:[{name:'grep',arguments:{pattern:'ABSENT_MATCH',path:'README.md'}}]},final(['README.md:1']),{text:'parent done'}])
  assert.deepEqual(payload(await f.run()).evidence_refs,[])
})

test('R31b legacy successful read records still normalize after child resume',async t=>{
  const f=await fixture(t,[delegate('explore'),{tool_calls:[{name:'read',arguments:{path:'./README.md'}}]},delegate('explore'),final(['README.md:1']),{text:'parent done'}])
  await f.request('setBudget',{max_rounds:2,max_total_tokens:0,max_seconds:0});const first=await f.run()
  assert.equal(first.result.exit_reason,'max_rounds')
  const child=first.detail.events.find(e=>e.type==='subagent_start').data.child_session_id
  const db=new DatabaseSync(f.db)
  db.prepare("UPDATE events SET data=json_remove(data,'$.observed_paths','$.name') WHERE session_id=? AND type='tool_call_result'").run(child);db.close()
  await f.request('resetSession');await f.request('getState',{sessionId:first.id})
  await f.request('setBudget',{max_rounds:0,max_total_tokens:0,max_seconds:0})
  assert.deepEqual(payload(await f.run({sessionId:first.id,text:'continue child'})).evidence_refs,['README.md:1'])
})
