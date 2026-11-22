import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import net from 'node:net'
import {test} from 'node:test'
import {fixture,pidAlive,until} from './helpers.mjs'

const call=command=>({tool_calls:[{name:'bash',arguments:{command}}]})
test('SEC2 dangerous commands require separate confirmation even with an explicit allow and bypass',async t=>{
  const f=await fixture(t,[call('rm -rf keep'),{text:'done'}])
  fs.mkdirSync(path.join(f.workspace,'keep'));fs.mkdirSync(path.join(f.workspace,'.minicode'));fs.writeFileSync(path.join(f.workspace,'.minicode/settings.json'),JSON.stringify({permissions:{allow:['bash(rm -rf:*)']}}))
  await f.request('setPermissionMode',{mode:'bypass'})
  const started=await f.request('sendPrompt',{workspace:f.workspace,text:'danger check',model:'fake'})
  const approval=await f.wait(e=>e.event==='approval');assert.equal(approval.request.tool_name,'dangerous_command');assert.ok(!approval.request.grant_rule)
  await f.request('resolveApproval',{sessionId:started.sessionId,approvalId:approval.approvalId,granted:false,remember:'session'})
  await f.wait(e=>e.event==='run_done');assert.ok(fs.existsSync(path.join(f.workspace,'keep')))
})

test('SEC2 explicit deny is rejected without any dangerous-command approval',async t=>{
  const f=await fixture(t,[call('rm -rf keep'),{text:'done'}])
  fs.mkdirSync(path.join(f.workspace,'keep'));fs.mkdirSync(path.join(f.workspace,'.minicode'));fs.writeFileSync(path.join(f.workspace,'.minicode/settings.json'),JSON.stringify({permissions:{deny:['bash(rm -rf:*)']}}))
  await f.request('setPermissionMode',{mode:'bypass'});const run=await f.run()
  assert.ok(!run.events.some(e=>e.event==='approval'));assert.ok(fs.existsSync(path.join(f.workspace,'keep')))
})

test('SEC2 dangerous confirmation cannot become a session grant',async t=>{
  const f=await fixture(t,[call('rm -rf disposable'),call('rm -rf disposable'),{text:'done'}])
  fs.mkdirSync(path.join(f.workspace,'disposable'));fs.mkdirSync(path.join(f.workspace,'.minicode'));fs.writeFileSync(path.join(f.workspace,'.minicode/settings.json'),JSON.stringify({permissions:{allow:['bash(rm -rf:*)']}}))
  await f.request('setPermissionMode',{mode:'bypass'})
  const started=await f.request('sendPrompt',{workspace:f.workspace,text:'confirm twice',model:'fake'})
  const first=await f.wait(e=>e.event==='approval');assert.equal(first.request.tool_name,'dangerous_command')
  await f.request('resolveApproval',{sessionId:started.sessionId,approvalId:first.approvalId,granted:true,remember:'session'})
  const second=await f.wait(e=>e.event==='approval'&&e.approvalId!==first.approvalId);assert.equal(second.request.tool_name,'dangerous_command')
  await f.request('resolveApproval',{sessionId:started.sessionId,approvalId:second.approvalId,granted:false});await f.wait(e=>e.event==='run_done')
  assert.ok(!fs.existsSync(path.join(f.workspace,'disposable')))
})

test('SEC2 an enabled OS sandbox permits workspace writes while rejecting writes outside it',{skip:process.platform!=='win32'},async t=>{
  const f=await fixture(t)
  const outside=path.join(f.root,'outside.txt')
  const command=`[IO.File]::WriteAllText('inside.txt','inside');try{[IO.File]::WriteAllText('${outside.replaceAll("'","''")}','escaped');Write-Output 'ESCAPED'}catch{Write-Output 'OUTSIDE_BLOCKED'}`
  fs.writeFileSync(f.script,JSON.stringify({turns:[{tool_calls:[{name:'powershell',arguments:{command}}]},{text:'done'}]}))
  fs.mkdirSync(path.join(f.workspace,'.minicode'));fs.writeFileSync(path.join(f.workspace,'.minicode/settings.json'),JSON.stringify({shellSandbox:{enabled:true,network:false}}))
  await f.request('setPermissionMode',{mode:'bypass'})
  const approvals=(async()=>{const approval=await f.wait(e=>e.event==='approval');await f.request('resolveApproval',{approvalId:approval.approvalId,sessionId:approval.sessionId,granted:true})})()
  const run=await f.run();await approvals
  const tool=run.detail.events.find(e=>e.type==='tool_call_result').data
  assert.equal(tool.success,true,JSON.stringify(tool))
  assert.equal(fs.readFileSync(path.join(f.workspace,'inside.txt'),'utf8'),'inside');assert.ok(!fs.existsSync(outside))
  assert.ok(run.detail.events.some(e=>e.type==='shell_sandbox'&&e.data.enforced===true&&e.data.network===false))
  assert.match(run.detail.events.find(e=>e.type==='tool_call_result').data.output_detail,/OUTSIDE_BLOCKED/)
})

test('SEC2 Windows sandbox blocks private file reads and network and omits inherited API secrets',{skip:process.platform!=='win32'},async t=>{
  const f=await fixture(t,[],{MINICODE_SANDBOX_TEST_SECRET:'PRIVATE_MUST_NOT_REACH_CHILD'})
  const outside=path.join(f.root,'private.txt');fs.writeFileSync(outside,'PRIVATE_FILE_MUST_NOT_REACH_CHILD')
  let connections=0;const server=net.createServer(socket=>{connections++;socket.end()})
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));t.after(()=>new Promise(resolve=>server.close(resolve)))
  const command=`try{[IO.File]::ReadAllText('${outside.replaceAll("'","''")}')}catch{Write-Output 'READ_BLOCKED'};$tcp=[Net.Sockets.TcpClient]::new();try{$task=$tcp.ConnectAsync('127.0.0.1',${server.address().port});if(!$task.Wait(700)){throw 'blocked'};Write-Output 'NETWORK_ESCAPED'}catch{Write-Output 'NETWORK_BLOCKED'}finally{$tcp.Dispose()};if([string]::IsNullOrEmpty($env:MINICODE_SANDBOX_TEST_SECRET)){Write-Output 'SECRET_OMITTED'}`
  fs.writeFileSync(f.script,JSON.stringify({turns:[{tool_calls:[{name:'powershell',arguments:{command}}]},{text:'done'}]}))
  fs.mkdirSync(path.join(f.workspace,'.minicode'));fs.writeFileSync(path.join(f.workspace,'.minicode/settings.json'),JSON.stringify({shellSandbox:{enabled:true}}))
  await f.request('setPermissionMode',{mode:'bypass'})
  const approvals=(async()=>{const approval=await f.wait(e=>e.event==='approval');await f.request('resolveApproval',{approvalId:approval.approvalId,sessionId:approval.sessionId,granted:true})})()
  const run=await f.run();await approvals
  const tool=run.detail.events.find(e=>e.type==='tool_call_result').data
  assert.equal(tool.success,true,JSON.stringify(tool));assert.match(tool.output_detail,/READ_BLOCKED/);assert.match(tool.output_detail,/NETWORK_BLOCKED/);assert.match(tool.output_detail,/SECRET_OMITTED/);assert.ok(!tool.output_detail.includes('MUST_NOT_REACH_CHILD'));assert.equal(connections,0)
})

test('SEC2 file deny rules cannot be bypassed through an approved arbitrary shell',async t=>{
  const f=await fixture(t,[call('cat .env'),{text:'done'}])
  fs.writeFileSync(path.join(f.workspace,'.env'),'PRIVATE_FILE_DENY_VALUE');fs.mkdirSync(path.join(f.workspace,'.minicode'));fs.writeFileSync(path.join(f.workspace,'.minicode/settings.json'),JSON.stringify({permissions:{deny:['read(.env)'],allow:['bash']}}))
  await f.request('setPermissionMode',{mode:'bypass'});const run=await f.run()
  const tool=run.detail.events.find(e=>e.type==='tool_call_result').data
  assert.equal(tool.success,false);assert.ok(!tool.output_detail.includes('PRIVATE_FILE_DENY_VALUE'));assert.ok(!run.events.some(e=>e.event==='approval'))
})

test('SEC2 unsupported Windows network configuration refuses execution and reports actual protection',{skip:process.platform!=='win32'},async t=>{
  const f=await fixture(t,[call('echo forbidden > forbidden.txt'),{text:'done'}])
  fs.mkdirSync(path.join(f.workspace,'.minicode'));fs.writeFileSync(path.join(f.workspace,'.minicode/settings.json'),JSON.stringify({shellSandbox:{enabled:true,network:true}}))
  await f.request('setPermissionMode',{mode:'bypass'})
  const approvals=(async()=>{const approval=await f.wait(e=>e.event==='approval');await f.request('resolveApproval',{approvalId:approval.approvalId,sessionId:approval.sessionId,granted:true})})()
  const run=await f.run();await approvals
  assert.ok(!fs.existsSync(path.join(f.workspace,'forbidden.txt')))
  assert.ok(run.detail.events.some(e=>e.type==='shell_sandbox'&&e.data.enforced===false&&e.data.command_executed===false))
  assert.ok(!run.detail.events.some(e=>e.type==='shell_sandbox'&&e.data.enforced===true))
})

test('SEC2 a writable child shell automatically uses OS isolation before parent review',{skip:process.platform!=='win32'},async t=>{
  const f=await fixture(t)
  await f.request('saveAgent',{agent:{name:'sandbox-writer',description:'sandbox fixture',instructions:'write a file',tools:['powershell'],inheritTools:false}})
  const command=`[IO.File]::WriteAllText('inside.txt','inside');try{[IO.File]::WriteAllText('${path.join(f.workspace,'escaped.txt').replaceAll("'","''")}','escaped')}catch{Write-Output 'PARENT_BLOCKED'}`
  fs.writeFileSync(f.script,JSON.stringify({turns:[{tool_calls:[{id:'d',name:'delegate',arguments:{kind:'sandbox-writer',task:'write',writable:true}}]},{tool_calls:[{name:'powershell',arguments:{command}}]},{text:JSON.stringify({summary:'done',findings:[],evidence_refs:[],unresolved:[]})},{tool_calls:[{name:'review_delegate',arguments:{delegate_call_id:'d',action:'discard'}}]},{text:'parent done'}]}))
  await f.request('setPermissionMode',{mode:'bypass'})
  const approvals=(async()=>{const approval=await f.wait(e=>e.event==='approval');await f.request('resolveApproval',{approvalId:approval.approvalId,sessionId:approval.sessionId,granted:true})})()
  const run=await f.run();await approvals
  const child=await f.request('getSession',{sessionId:run.detail.events.find(e=>e.type==='subagent_start').data.child_session_id})
  const shell=child.events.find(e=>e.type==='tool_call_result'&&e.data.name==='powershell').data
  assert.equal(shell.success,true,JSON.stringify(shell));assert.match(shell.output_detail,/PARENT_BLOCKED/)
  assert.ok(child.events.some(e=>e.type==='shell_sandbox'&&e.data.enforced===true));assert.ok(!fs.existsSync(path.join(f.workspace,'inside.txt')));assert.ok(!fs.existsSync(path.join(f.workspace,'escaped.txt')))
})

test('SEC2 cancelling an idle sandboxed background job reaps the restricted process',{skip:process.platform!=='win32'},async t=>{
  const f=await fixture(t,[{tool_calls:[{name:'powershell',arguments:{command:"[IO.File]::WriteAllText('sandbox.pid',[string]$PID);while($true){Start-Sleep -Milliseconds 50}",background:true}}]},{text:'started'}])
  fs.mkdirSync(path.join(f.workspace,'.minicode'));fs.writeFileSync(path.join(f.workspace,'.minicode/settings.json'),JSON.stringify({shellSandbox:{enabled:true}}))
  await f.request('setPermissionMode',{mode:'bypass'})
  const approvals=(async()=>{const approval=await f.wait(e=>e.event==='approval');await f.request('resolveApproval',{approvalId:approval.approvalId,sessionId:approval.sessionId,granted:true})})()
  const run=await f.run();await approvals
  const marker=path.join(f.workspace,'sandbox.pid');assert.ok(await until(()=>fs.existsSync(marker)))
  const pid=Number(fs.readFileSync(marker,'utf8'));assert.ok(pidAlive(pid))
  await f.request('cancelTurn',{sessionId:run.id});assert.ok(await until(()=>!pidAlive(pid)))
})

test('SEC2 invalid shellSandbox.readOnlyPaths report clear normalization errors before execution',async t=>{
  for(const [entry,pattern] of [
    ['./tools',/shellSandbox\.readOnlyPaths.*absolute path/],
    ['%USERPROFILE%\\tools',/shellSandbox\.readOnlyPaths.*unexpanded environment variable/],
    ['$HOME/tools',/shellSandbox\.readOnlyPaths.*unexpanded environment variable/]
  ]){
    const f=await fixture(t,[call('echo should-not-run'),{text:'done'}])
    fs.mkdirSync(path.join(f.workspace,'.minicode'))
    fs.writeFileSync(path.join(f.workspace,'.minicode/settings.json'),JSON.stringify({shellSandbox:{enabled:true,readOnlyPaths:[entry]}}))
    await assert.rejects(
      f.request('sendPrompt',{workspace:f.workspace,model:'fake',text:'validate sandbox paths'}),
      pattern
    )
  }
})

test('SEC2 Windows sandbox authorizes configured readOnlyPaths while blocking writes and other outside reads',{skip:process.platform!=='win32'},async t=>{
  const f=await fixture(t)
  const outsideAllowed=path.join(f.root,'ext-readonly')
  const outsideBlocked=path.join(f.root,'ext-blocked')
  fs.mkdirSync(outsideAllowed);fs.mkdirSync(outsideBlocked)
  fs.writeFileSync(path.join(outsideAllowed,'allowed.txt'),'ALLOWED_TOOL_DATA')
  fs.writeFileSync(path.join(outsideBlocked,'blocked.txt'),'BLOCKED_SECRET')
  const allowedFile=path.join(outsideAllowed,'allowed.txt').replaceAll("'","''")
  const writeAttempt=path.join(outsideAllowed,'write.txt').replaceAll("'","''")
  const blockedFile=path.join(outsideBlocked,'blocked.txt').replaceAll("'","''")
  const command=`Write-Output ([IO.File]::ReadAllText('${allowedFile}'));try{[IO.File]::WriteAllText('${writeAttempt}','nope');Write-Output 'WRITE_ESCAPED'}catch{Write-Output 'WRITE_BLOCKED'};try{[IO.File]::ReadAllText('${blockedFile}');Write-Output 'READ_ESCAPED'}catch{Write-Output 'READ_BLOCKED'}`
  fs.writeFileSync(f.script,JSON.stringify({turns:[{tool_calls:[{name:'powershell',arguments:{command}}]},{text:'done'}]}))
  fs.mkdirSync(path.join(f.workspace,'.minicode'))
  fs.writeFileSync(path.join(f.workspace,'.minicode/settings.json'),JSON.stringify({shellSandbox:{enabled:true,network:false,readOnlyPaths:[outsideAllowed]}}))
  await f.request('setPermissionMode',{mode:'bypass'})
  const approvals=(async()=>{const approval=await f.wait(e=>e.event==='approval');await f.request('resolveApproval',{approvalId:approval.approvalId,sessionId:approval.sessionId,granted:true})})()
  const run=await f.run();await approvals
  const tool=run.detail.events.find(e=>e.type==='tool_call_result').data
  assert.equal(tool.success,true,JSON.stringify(tool))
  assert.match(tool.output_detail,/ALLOWED_TOOL_DATA/)
  assert.match(tool.output_detail,/WRITE_BLOCKED/)
  assert.match(tool.output_detail,/READ_BLOCKED/)
  assert.ok(!fs.existsSync(path.join(outsideAllowed,'write.txt')))
})
