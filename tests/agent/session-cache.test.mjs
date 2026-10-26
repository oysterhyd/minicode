import assert from 'node:assert/strict'
import {test} from 'node:test'
import {fixture} from './helpers.mjs'
test('B21: eviction preserves session mode, effort and remembered grants across lazy and runtime reloads',async t=>{
 const f=await fixture(t,[{tool_calls:[{name:'write',arguments:{path:'cache.txt',content:'once'}}]},{text:'done'}])
 const first=await f.request('sendPrompt',{clientKey:'view0',workspace:f.workspace,model:'fake',text:'first'})
 const approval=await f.wait(e=>e.event==='approval'&&e.sessionId===first.sessionId)
 await f.request('resolveApproval',{sessionId:first.sessionId,approvalId:approval.approvalId,granted:true,remember:'session'})
 await f.wait(e=>e.event==='run_done'&&e.sessionId===first.sessionId)
 await f.request('setPermissionMode',{clientKey:'view0',sessionId:first.sessionId,mode:'bypass'})
 await f.request('setEffort',{clientKey:'view0',sessionId:first.sessionId,effort:'high'})
 for(let i=1;i<=7;i++) {
  await f.request('setPermissionMode',{clientKey:`view${i}`,mode:'bypass'})
  await f.run({clientKey:`view${i}`})
 }
 assert.equal((await f.request('getState',{clientKey:'view0'})).sessionId,null,'fixture did not evict first session')
 const restored=await f.request('selectSession',{clientKey:'view0',sessionId:first.sessionId,deferRuntime:true})
 assert.equal(restored.permissionMode,'bypass');assert.equal(restored.effort,'high');assert.deepEqual(restored.alwaysAllow,['write'])
 await f.run({clientKey:'view0',sessionId:first.sessionId,text:'after cache eviction'})
 assert.deepEqual((await f.request('getState',{clientKey:'view0',sessionId:first.sessionId})).alwaysAllow,['write'])
})
