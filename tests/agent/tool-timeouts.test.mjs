import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import {test} from 'node:test'
import {fixture} from './helpers.mjs'

for (const mode of ['service', 'tool', 'success', 'long']) {
  test(`MCP configurable timeout: ${mode}`, async t => {
    const f = await fixture(t)
    const script = path.join(f.root, 'slow.cjs')
    fs.writeFileSync(script, `require('readline').createInterface({input:process.stdin}).on('line',line=>{const r=JSON.parse(line);if(r.id===undefined)return;const result=r.method==='initialize'?{protocolVersion:'2025-11-25',capabilities:{tools:{}},serverInfo:{name:'slow',version:'1'}}:r.method==='tools/list'?{tools:[{name:'slow',inputSchema:{type:'object'}}]}:{content:[{type:'text',text:'finished'}]};setTimeout(()=>console.log(JSON.stringify({jsonrpc:'2.0',id:r.id,result})),r.method==='tools/call'?1000:0)})`)
    fs.writeFileSync(path.join(f.workspace, '.mcp.json'), JSON.stringify({mcpServers:{slow:{command:process.execPath,args:[script],timeout_s:mode === 'service' ? 0.6 : mode === 'long' ? 600 : 2}}}))
    const entry = (await f.request('getLocalExtensions', {workspace:f.workspace})).entries.find(e => e.kind === 'mcp')
    await f.request('setLocalExtensionEnabled', {workspace:f.workspace,id:entry.id,enabled:true})
    const discovery = await f.request('refreshMcp', {workspace:f.workspace})
    assert.equal(discovery.servers[0].error, null)
    const name = discovery.tools[0]
    if (mode === 'tool') {
      fs.mkdirSync(path.join(f.root,'.minicode'), {recursive:true})
      const configPath = path.join(f.root,'.minicode/desktop-config.json')
      const config = JSON.parse(fs.readFileSync(configPath,'utf8'))
      config.toolTimeouts = {[name]:0.1}
      fs.writeFileSync(configPath,JSON.stringify(config))
    }
    fs.writeFileSync(f.script,JSON.stringify({turns:[{tool_calls:[{name,arguments:{}}]},{text:'done'}]}))
    await f.request('setPermissionMode',{mode:'bypass'})
    const result = await f.run()
    const output = result.detail.events.find(e => e.type === 'tool_call_result').data
    assert.equal(output.success, mode === 'success' || mode === 'long')
    if (!output.success) assert.match(output.error,/timed out/)
  })
}
