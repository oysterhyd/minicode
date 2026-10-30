import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import {test} from 'node:test'
import {fixture,connect} from './helpers.mjs'

const done={text:JSON.stringify({summary:'done',findings:[],evidence_refs:[],unresolved:[]})}
const delegate=(writable=true)=>({tool_calls:[{id:'fix',name:'delegate',arguments:{kind:'fixer',task:'change files',writable}}]})
const write=(name='child.txt',content='isolated')=>({tool_calls:[{name:'write',arguments:{path:name,content}}]})
const review=action=>({tool_calls:[{name:'review_delegate',arguments:{delegate_call_id:'fix',action}}]})
const result=(run,name)=>run.detail.events.filter(e=>e.type==='tool_call_result'&&e.data.name===name).at(-1)?.data

test('A4 default read-only child cannot write files or run a shell even in bypass',async t=>{
  const f=await fixture(t,[delegate(false),write(),{tool_calls:[{name:'bash',arguments:{command:'echo unsafe > shell.txt'}}]},done,{text:'parent done'}])
  await f.request('setPermissionMode',{mode:'bypass'});const run=await f.run()
  assert.ok(!fs.existsSync(path.join(f.workspace,'child.txt')))
  assert.ok(!fs.existsSync(path.join(f.workspace,'shell.txt')))
  const id=run.detail.events.find(e=>e.type==='subagent_start').data.child_session_id
  const child=await f.request('getSession',{sessionId:id})
  assert.ok(child.events.filter(e=>e.type==='tool_call_result'&&['write','bash'].includes(e.data.name)).every(e=>e.data.success===false))
})

test('A4 isolated writes remain outside the main workspace and can be discarded',async t=>{
  const f=await fixture(t,[delegate(),write(),done,review('inspect'),review('discard'),{text:'parent done'}])
  await f.request('setPermissionMode',{mode:'bypass'});const run=await f.run()
  assert.ok(!fs.existsSync(path.join(f.workspace,'child.txt')))
  assert.equal(result(run,'review_delegate').success,true)
  const inspection=run.detail.events.find(e=>e.type==='tool_call_result'&&e.data.name==='review_delegate'&&e.data.output_detail.includes('child.txt'))
  assert.ok(inspection,'parent must receive a diff containing the child changes')
  const child=await f.request('getSession',{sessionId:run.detail.events.find(e=>e.type==='subagent_start').data.child_session_id})
  assert.notEqual(path.resolve(child.summary.workspace),path.resolve(f.workspace))
  assert.ok(!fs.existsSync(child.summary.workspace),'discard cleans up the isolated copy')
})

for(const mode of ['bypass','accept_edits']) for(const granted of [true,false]) {
  test(`A4 ${mode} merge requires a reviewed diff and explicit approval (${granted})`,async t=>{
    const f=await fixture(t,[delegate(),write(),done,review('inspect'),review('merge'),{text:'parent done'}])
    await f.request('setPermissionMode',{mode})
    const started=await f.request('sendPrompt',{workspace:f.workspace,text:'isolate then review',model:'fake'})
    const approval=await f.wait(e=>e.event==='approval'&&e.request.tool_name==='review_delegate')
    assert.equal(approval.sessionId,started.sessionId)
    assert.ok(!fs.existsSync(path.join(f.workspace,'child.txt')))
    await f.request('resolveApproval',{sessionId:started.sessionId,approvalId:approval.approvalId,granted})
    await f.wait(e=>e.event==='run_done')
    assert.equal(fs.existsSync(path.join(f.workspace,'child.txt')),granted)
    if(granted)assert.equal(fs.readFileSync(path.join(f.workspace,'child.txt'),'utf8'),'isolated')
    const detail=await f.request('getSession',{sessionId:started.sessionId})
    const child=await f.request('getSession',{sessionId:detail.events.find(e=>e.type==='subagent_start').data.child_session_id})
    assert.ok(!fs.existsSync(child.summary.workspace),'merge or rejection cleans up the isolated copy')
  })
}

test('A4 dirty files are copied and a conflicting parent edit prevents all merge writes',async t=>{
  const f=await fixture(t,[delegate(),{tool_calls:[{name:'read',arguments:{path:'README.md'}}]},write('README.md','child replacement'),write(),done,review('inspect'),review('merge'),{text:'parent done'}])
  fs.writeFileSync(path.join(f.workspace,'README.md'),'uncommitted parent content')
  await f.request('setPermissionMode',{mode:'bypass'})
  const started=await f.request('sendPrompt',{workspace:f.workspace,text:'isolate then review',model:'fake'})
  const approval=await f.wait(e=>e.event==='approval'&&e.request.tool_name==='review_delegate')
  assert.equal(fs.readFileSync(path.join(f.workspace,'README.md'),'utf8'),'uncommitted parent content')
  fs.writeFileSync(path.join(f.workspace,'README.md'),'new external parent edit')
  await f.request('resolveApproval',{sessionId:started.sessionId,approvalId:approval.approvalId,granted:true})
  await f.wait(e=>e.event==='run_done')
  assert.equal(fs.readFileSync(path.join(f.workspace,'README.md'),'utf8'),'new external parent edit')
  assert.ok(!fs.existsSync(path.join(f.workspace,'child.txt')))
  const detail=await f.request('getSession',{sessionId:started.sessionId})
  assert.equal(result({detail},'review_delegate').success,false)
  const child=await f.request('getSession',{sessionId:detail.events.find(e=>e.type==='subagent_start').data.child_session_id})
  assert.ok(child.events.some(e=>e.type==='tool_call_result'&&e.data.name==='read'&&e.data.output_detail.includes('uncommitted parent content')))
})

test('A4 merge cannot skip review or approve an unseen diff in the same model response',async t=>{
  const f=await fixture(t,[delegate(),write(),done,review('merge'),{tool_calls:[...review('inspect').tool_calls,...review('merge').tool_calls]},review('discard'),{text:'parent done'}])
  await f.request('setPermissionMode',{mode:'bypass'});const run=await f.run()
  const merges=run.detail.events.filter(e=>e.type==='tool_call_start'&&e.data.name==='review_delegate'&&e.data.arguments.action==='merge')
  assert.equal(merges.length,2)
  assert.ok(merges.every(call=>run.detail.events.find(e=>e.type==='tool_call_result'&&e.data.call_id===call.data.call_id)?.data.success===false))
  assert.ok(!run.events.some(e=>e.event==='approval'))
  assert.ok(!fs.existsSync(path.join(f.workspace,'child.txt')))
})

test('A4 writable child shell is refused when OS isolation is unavailable',async t=>{
  const f=await fixture(t,[delegate(),{tool_calls:[{name:'bash',arguments:{command:'echo unsafe > shell.txt'}}]},done,review('discard'),{text:'parent done'}])
  await f.request('setPermissionMode',{mode:'bypass'});const run=await f.run()
  const child=await f.request('getSession',{sessionId:run.detail.events.find(e=>e.type==='subagent_start').data.child_session_id})
  const shell=child.events.find(e=>e.type==='tool_call_result'&&e.data.name==='bash')
  assert.equal(shell.data.success,false);assert.match(shell.data.error,/sandbox|沙箱|隔离/i)
  assert.ok(!fs.existsSync(path.join(f.workspace,'shell.txt')))
})

test('A4 cancellation cleans an isolated child without changing parent files',async t=>{
  const f=await fixture(t,[delegate(),write(),done,{text:'parent done'}])
  const started=await f.request('sendPrompt',{workspace:f.workspace,text:'isolate',model:'fake'})
  await f.wait(e=>e.event==='approval')
  const childId=f.events.find(e=>e.event==='agent_event'&&e.item.type==='subagent_start').item.data.child_session_id
  const child=await f.request('getSession',{sessionId:childId})
  await f.request('cancelTurn',{sessionId:started.sessionId});await f.wait(e=>e.event==='run_done')
  assert.ok(!fs.existsSync(child.summary.workspace));assert.ok(!fs.existsSync(path.join(f.workspace,'child.txt')))
})

test('A4 opening a child directly preserves its read-only boundary',async t=>{
  const f=await fixture(t,[delegate(false),done,{text:'parent done'}])
  await f.request('setPermissionMode',{mode:'bypass'});const first=await f.run()
  const id=first.detail.events.find(e=>e.type==='subagent_start').data.child_session_id
  await f.request('resetSession')
  fs.writeFileSync(f.script,JSON.stringify({turns:[write('escape.txt'),done]}))
  const run=await f.run({sessionId:id,text:'attempt write'})
  assert.ok(!fs.existsSync(path.join(f.workspace,'escape.txt')))
  assert.ok(run.detail.events.some(e=>e.type==='tool_call_result'&&e.data.name==='write'&&e.data.success===false))
})

test('A4 parent process recovery retains a pending reviewed snapshot',async t=>{
  const f=await fixture(t,[delegate(),write(),done,review('inspect'),review('merge'),{text:'parent done'}])
  await f.request('setPermissionMode',{mode:'bypass'});await f.request('setBudget',{max_rounds:4,max_total_tokens:0,max_seconds:0})
  const first=await f.run()
  assert.equal(first.result.exit_reason,'max_rounds')
  const child=await f.request('getSession',{sessionId:first.detail.events.find(e=>e.type==='subagent_start').data.child_session_id})
  assert.ok(fs.existsSync(child.summary.workspace))
  const exited=new Promise(resolve=>f.child.once('exit',resolve));f.child.kill('SIGKILL');await exited
  const c=connect(f)
  await c.request('getState',{sessionId:first.id});await c.request('setPermissionMode',{mode:'bypass'});await c.request('setBudget',{max_rounds:0,max_total_tokens:0,max_seconds:0})
  await c.request('sendPrompt',{sessionId:first.id,workspace:f.workspace,text:'review recovered changes',model:'fake'})
  const approval=await c.wait(e=>e.event==='approval'&&e.request.tool_name==='review_delegate')
  assert.ok(!fs.existsSync(path.join(f.workspace,'child.txt')))
  await c.request('resolveApproval',{sessionId:first.id,approvalId:approval.approvalId,granted:false});await c.wait(e=>e.event==='run_done')
  assert.ok(!fs.existsSync(child.summary.workspace))
})

test('A4 snapshot creation failure leaves the main workspace unchanged',async t=>{
  const f=await fixture(t,[delegate(),write(),done,{text:'parent done'}])
  fs.writeFileSync(path.join(f.root,'delegates'),'blocked storage')
  await f.request('setPermissionMode',{mode:'bypass'});const run=await f.run()
  assert.notEqual(run.result.exit_reason,'completed')
  assert.ok(!fs.existsSync(path.join(f.workspace,'child.txt')))
  assert.ok(!run.detail.events.some(e=>e.type==='subagent_start'))
})

test('A4 a paused writable child resumes the same isolated workspace after process recovery',async t=>{
  const second={tool_calls:[{...delegate().tool_calls[0],id:'fix2'}]}
  const inspect=review('inspect');inspect.tool_calls[0].arguments.delegate_call_id='fix2'
  const discard=review('discard');discard.tool_calls[0].arguments.delegate_call_id='fix2'
  const f=await fixture(t,[delegate(),write(),second,done,inspect,discard,{text:'parent done'}])
  await f.request('setPermissionMode',{mode:'bypass'});await f.request('setBudget',{max_rounds:2,max_total_tokens:0,max_seconds:0})
  const first=await f.run();assert.equal(first.result.exit_reason,'max_rounds')
  const original=first.detail.events.find(e=>e.type==='subagent_start').data.child_session_id
  const exited=new Promise(resolve=>f.child.once('exit',resolve));f.child.kill('SIGKILL');await exited
  const c=connect(f);await c.request('getState',{sessionId:first.id});await c.request('setPermissionMode',{mode:'bypass'});await c.request('setBudget',{max_rounds:0,max_total_tokens:0,max_seconds:0})
  const next=await c.run({sessionId:first.id,text:'resume isolated child'})
  assert.equal(next.detail.events.filter(e=>e.type==='subagent_start').at(-1).data.child_session_id,original)
  assert.equal(result(next,'review_delegate').success,true)
  assert.ok(!fs.existsSync(path.join(f.workspace,'child.txt')))
})

test('A4 a large reviewed diff is available as a complete archived artifact',async t=>{
  const content=Array.from({length:8000},(_,i)=>`changed line ${i}`).join('\n')
  const f=await fixture(t,[delegate(),write('large.txt',content),done,review('inspect'),review('discard'),{text:'parent done'}])
  await f.request('setPermissionMode',{mode:'bypass'});const run=await f.run()
  const inspected=run.detail.events.find(e=>e.type==='tool_call_result'&&e.data.name==='review_delegate'&&e.data.artifact_id)
  assert.ok(inspected,'review output should reuse the existing artifact flow')
  assert.equal(inspected.data.truncated,true)
  let offset=0,full=''
  for(let page=0;page<20;page++) {
    const archive=await f.request('readArtifact',{sessionId:run.id,artifactId:inspected.data.artifact_id,offset})
    full+=archive.text;offset+=Array.from(archive.text).length
    if(!archive.hasMore)break
  }
  assert.ok(full.includes('changed line 7999'))
  assert.ok(!fs.existsSync(path.join(f.workspace,'large.txt')))
})

test('A4 failed snapshot cleanup retains cross-process ownership until cleanup succeeds',async t=>{
  const f=await fixture(t,[delegate(),write(),done,{text:'parent done'}])
  await f.request('setPermissionMode',{mode:'bypass'});const first=await f.run()
  const child=await f.request('getSession',{sessionId:first.detail.events.find(e=>e.type==='subagent_start').data.child_session_id})
  const root=path.dirname(child.summary.workspace),backup=root+'.backup'
  fs.renameSync(root,backup);fs.symlinkSync(f.workspace,root,process.platform==='win32'?'junction':'dir')
  const c=connect(f)
  try {
    await assert.rejects(f.request('resetSession'),/linked isolation/)
    await assert.rejects(c.request('deleteSession',{sessionId:first.id}),/in use|running|占用|运行|持有|locked/i)
    assert.ok(fs.existsSync(path.join(f.workspace,'README.md')))
  } finally {fs.unlinkSync(root);fs.renameSync(backup,root)}
  await f.request('resetSession')
  await c.request('deleteSession',{sessionId:first.id})
})

test('A4 deleting a session after an owner crash removes its retained snapshots',async t=>{
  const f=await fixture(t,[delegate(),write(),done,{text:'parent done'}])
  await f.request('setPermissionMode',{mode:'bypass'});const first=await f.run()
  const child=await f.request('getSession',{sessionId:first.detail.events.find(e=>e.type==='subagent_start').data.child_session_id})
  const root=path.dirname(child.summary.workspace)
  const exited=new Promise(resolve=>f.child.once('exit',resolve));f.child.kill('SIGKILL');await exited
  const c=connect(f);await c.request('deleteSession',{sessionId:first.id})
  assert.ok(!fs.existsSync(root));assert.ok(!fs.existsSync(path.join(f.workspace,'child.txt')))
})
