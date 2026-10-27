import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import {test} from 'node:test'
import {fixture} from './helpers.mjs'

test('MCP startup failures retain bounded, redacted stderr and a diagnostic log',async t=>{
  const f=await fixture(t)
  const script=path.join(f.root,'failing-server.cjs')
  const secret='mcp-private-token-fixture'
  fs.writeFileSync(script,`for(let i=0;i<250;i++)process.stderr.write('diagnostic '+i+'\\n');process.stderr.write('fatal: missing config '+process.env.MCP_TEST_TOKEN+'\\n')`)
  fs.writeFileSync(path.join(f.workspace,'.mcp.json'),JSON.stringify({mcpServers:{broken:{command:process.execPath,args:[script],env:{MCP_TEST_TOKEN:secret}}}}))
  const entry=(await f.request('getLocalExtensions',{workspace:f.workspace})).entries.find(e=>e.kind==='mcp'&&e.name==='broken')
  await f.request('setLocalExtensionEnabled',{workspace:f.workspace,id:entry.id,enabled:true})
  const {servers}=await f.request('refreshMcp',{workspace:f.workspace})
  const status=servers[0]
  assert.match(status.error,/fatal: missing config/)
  assert.match(status.error,/diagnostic 249/)
  assert.doesNotMatch(status.error,/diagnostic 0\n/)
  assert.ok(status.error.length<65536)
  assert.ok(status.stderr_tail.includes('fatal: missing config'))
  assert.ok(status.log_path)
  const log=fs.readFileSync(status.log_path,'utf8')
  assert.match(log,/diagnostic 0/)
  assert.match(log,/fatal: missing config/)
  assert.ok(!log.includes(secret))
  assert.ok(!JSON.stringify(status).includes(secret))
})

test('MCP drains large stderr streams without blocking discovery',async t=>{
  const f=await fixture(t)
  const script=path.join(f.root,'noisy-server.cjs')
  fs.writeFileSync(script,`process.stderr.write('x'.repeat(2*1024*1024)+'\\nlast diagnostic\\n')`)
  fs.writeFileSync(path.join(f.workspace,'.mcp.json'),JSON.stringify({mcpServers:{noisy:{command:process.execPath,args:[script]}}}))
  const entry=(await f.request('getLocalExtensions',{workspace:f.workspace})).entries.find(e=>e.kind==='mcp'&&e.name==='noisy')
  await f.request('setLocalExtensionEnabled',{workspace:f.workspace,id:entry.id,enabled:true})
  const {servers}=await f.request('refreshMcp',{workspace:f.workspace})
  assert.match(servers[0].error,/last diagnostic/)
  assert.ok(servers[0].stderr_tail.length<65536)
  assert.ok(fs.statSync(servers[0].log_path).size<=1_000_000)
  assert.match(fs.readFileSync(servers[0].log_path,'utf8'),/last diagnostic/)
})

test('successful MCP discovery exposes its recent stderr and log path',async t=>{
  const f=await fixture(t)
  const script=path.join(f.root,'working-server.cjs')
  fs.writeFileSync(script,`const rl=require('node:readline').createInterface({input:process.stdin});rl.on('line',line=>{const r=JSON.parse(line);if(r.id===undefined)return;process.stderr.write('server is ready\\n');const result=r.method==='initialize'?{protocolVersion:'2025-11-25',serverInfo:{name:'working',version:'1'}}:{tools:[]};process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:r.id,result})+'\\n')})`)
  fs.writeFileSync(path.join(f.workspace,'.mcp.json'),JSON.stringify({mcpServers:{working:{command:process.execPath,args:[script]}}}))
  const entry=(await f.request('getLocalExtensions',{workspace:f.workspace})).entries.find(e=>e.kind==='mcp'&&e.name==='working')
  await f.request('setLocalExtensionEnabled',{workspace:f.workspace,id:entry.id,enabled:true})
  const {servers}=await f.request('refreshMcp',{workspace:f.workspace})
  assert.equal(servers[0].error,null)
  assert.match(servers[0].stderr_tail,/server is ready/)
  assert.match(fs.readFileSync(servers[0].log_path,'utf8'),/server is ready/)
})
