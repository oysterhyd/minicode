import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import {spawnSync} from 'node:child_process'
import {DatabaseSync} from 'node:sqlite'
import {test} from 'node:test'
import {fixture,connect} from './helpers.mjs'
import {sseMock,openaiFrames,configureService} from './sse-mock.mjs'

const hasGitBash=process.platform==='win32' && (process.env.PATH || '').split(';').some(dir=>
  fs.existsSync(path.join(dir,'git.exe')) && ['../bin/bash.exe','../usr/bin/bash.exe'].some(candidate=>fs.existsSync(path.resolve(dir,candidate))))
const hasPwsh7=process.platform==='win32' && spawnSync('pwsh.exe',['-NoProfile','-Command','$PSVersionTable.PSVersion.Major'],{encoding:'utf8',windowsHide:true}).stdout?.trim()==='7'

function legacyPath() {
  const root=process.env.SystemRoot || 'C:\\Windows'
  return [path.dirname(process.execPath),path.join(root,'System32'),path.join(root,'System32','WindowsPowerShell','v1.0')].join(';')
}

test('Windows prompt names PowerShell 5.1 and its command-chain limitation', {skip:process.platform!=='win32'}, async t=>{
  const f=await fixture(t,[],{PATH:legacyPath(),MINICODE_WINDOWS_SHELL:'powershell'})
  const mock=await sseMock(t,[{frames:openaiFrames({text:'done'}),done:true}])
  await f.run({model:await configureService(f,mock.url,'openai')})
  const system=mock.payloads[0].messages.find(message=>message.role==='system').content
  assert.match(system,/PowerShell\s+5\.1/)
  assert.match(system,/不支持[^\n]*&&/)
  assert.match(system,/bash[^\n]*PowerShell/)
})

test('Windows prompt distinguishes Git Bash from PowerShell 7', {skip:!hasGitBash || !hasPwsh7}, async t=>{
  const f=await fixture(t,[],{MINICODE_WINDOWS_SHELL:''})
  const mock=await sseMock(t,[{frames:openaiFrames({text:'done'}),done:true}])
  await f.run({model:await configureService(f,mock.url,'openai')})
  const system=mock.payloads[0].messages.find(message=>message.role==='system').content
  assert.match(system,/bash 工具使用 Git Bash/)
  assert.match(system,/PowerShell\s+7\./)
  assert.match(system,/PowerShell 7 支持 &&/)
})

test('older frozen Windows prompts receive current shell guidance on resume', {skip:process.platform!=='win32'}, async t=>{
  const f=await fixture(t,[],{PATH:legacyPath(),MINICODE_WINDOWS_SHELL:'powershell'})
  const mock=await sseMock(t,[{frames:openaiFrames({text:'done'}),done:true}])
  const model=await configureService(f,mock.url,'openai')
  const first=await f.run({model})
  await f.close()
  const db=new DatabaseSync(f.db)
  db.prepare('UPDATE session_prompts SET system=?, dynamic_hash=NULL WHERE session_id=?').run('旧提示：bash 工具实际运行 PowerShell',first.id)
  db.close()
  const resumed=connect(f,{PATH:legacyPath(),MINICODE_WINDOWS_SHELL:'powershell'})
  t.after(()=>resumed.close())
  await resumed.run({sessionId:first.id,model,text:'继续'})
  const payload=mock.payloads[1]
  assert.equal(payload.messages.find(message=>message.role==='system').content,'旧提示：bash 工具实际运行 PowerShell')
  assert.ok(payload.messages.some(message=>message.role==='user' && JSON.stringify(message.content).includes('PowerShell 5.1')))
  await resumed.run({sessionId:first.id,model,text:'继续检查'})
  assert.equal(mock.payloads[2].messages.filter(message=>message.role==='user' && JSON.stringify(message.content).includes('当前命令环境')).length,1)
})
