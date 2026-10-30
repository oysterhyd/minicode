import assert from 'node:assert/strict'
import {test} from 'node:test'
import {fixture} from './helpers.mjs'

test('Windows shell children default Python to UTF-8', {skip:process.platform!=='win32'}, async t=>{
  const command='node -e "console.log(process.env.PYTHONUTF8,process.env.PYTHONIOENCODING)"'
  const f=await fixture(t,[{tool_calls:[{name:'bash',arguments:{command}},{name:'powershell',arguments:{command}}]},{text:'done'}],{PYTHONUTF8:'',PYTHONIOENCODING:''})
  await f.request('setPermissionMode',{mode:'bypass'})
  const result=await f.run()
  const tools=result.detail.events.filter(e=>e.type==='tool_call_result')
  assert.equal(tools.length,2)
  for (const tool of tools) assert.match(tool.data.output_detail,/1 utf-8/i)
})

test('Windows shell children preserve explicit Python encoding preferences', {skip:process.platform!=='win32'}, async t=>{
  const command='node -e "console.log(process.env.PYTHONUTF8,process.env.PYTHONIOENCODING)"'
  const f=await fixture(t,[{tool_calls:[{name:'powershell',arguments:{command}}]},{text:'done'}],{PYTHONUTF8:'0',PYTHONIOENCODING:'latin1'})
  await f.request('setPermissionMode',{mode:'bypass'})
  const result=await f.run()
  assert.match(result.detail.events.find(e=>e.type==='tool_call_result').data.output_detail,/0 latin1/)
})

test('PowerShell console output encoding uses UTF-8 without a BOM', {skip:process.platform!=='win32'}, async t=>{
  const f=await fixture(t,[{tool_calls:[{name:'powershell',arguments:{command:'[Console]::OutputEncoding.GetPreamble().Length'}}]},{text:'done'}])
  await f.request('setPermissionMode',{mode:'bypass'})
  const approvals=(async()=>{const approval=await f.wait(e=>e.event==='approval');assert.equal(approval.request.tool_name,'powershell');assert.equal(approval.request.grant_rule,null);await f.request('resolveApproval',{approvalId:approval.approvalId,sessionId:approval.sessionId,granted:true})})()
  const result=await f.run()
  await approvals
  assert.equal(result.detail.events.find(e=>e.type==='tool_call_result').data.output_detail.trim(),'0')
})
