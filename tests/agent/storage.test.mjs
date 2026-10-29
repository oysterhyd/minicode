import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import {DatabaseSync} from 'node:sqlite'
import {test} from 'node:test'
import {fixture} from './helpers.mjs'

test('R28: persisted events contain previews and references while old detail consumers still work', async t => {
  const f = await fixture(t, [{tool_calls:[
    {name:'read',arguments:{path:'README.md'}},
    {name:'bash',arguments:{command:'node -e "process.stdout.write(\'z\'.repeat(9000))"'}}
  ]},{text:'done'}])
  fs.writeFileSync(path.join(f.workspace,'README.md'),'The word truncated is ordinary complete file content.')
  await f.request('setPermissionMode',{mode:'bypass'})
  const r = await f.run()
  const read = r.detail.events.find(e => e.type === 'tool_call_result' && e.data.name === 'read')
  assert.equal(read.data.truncated,false)
  const command = r.detail.events.find(e => e.type === 'tool_call_result' && e.data.name === 'bash')
  assert.equal(command.data.output_detail,'z'.repeat(9000))
  assert.equal(r.events.find(e => e.item?.type === 'tool_call_result' && e.item.data.name === 'bash').item.data.output_detail,'z'.repeat(9000))
  const db = new DatabaseSync(f.db,{readOnly:true}); t.after(()=>{try {db.close()} catch {}})
  const stored = JSON.parse(db.prepare("SELECT data FROM events WHERE session_id=? AND type='tool_call_result' AND json_extract(data,'$.name')='bash'").get(r.id).data)
  assert.equal(stored.output_detail,undefined)
  assert.equal(stored.result_ref,command.data.call_id)
  assert.ok(JSON.stringify(stored).length < 2000)
  const journal = JSON.parse(db.prepare('SELECT result FROM tool_results WHERE session_id=? AND call_id=?').get(r.id,command.data.call_id).result)
  db.close()
  assert.equal(journal.content,'z'.repeat(9000))
})

test('R28: failed MCP tool results retain their images in model history and restored detail', async t => {
  const f = await fixture(t)
  const image = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Wl6x1cAAAAASUVORK5CYII='
  const server = path.join(f.root,'image-error.cjs')
  fs.writeFileSync(server,`require('readline').createInterface({input:process.stdin}).on('line',line=>{const r=JSON.parse(line);if(r.id===undefined)return;const result=r.method==='initialize'?{protocolVersion:'2025-11-25',capabilities:{tools:{}},serverInfo:{name:'image-error',version:'1'}}:r.method==='tools/list'?{tools:[{name:'inspect',inputSchema:{type:'object'}}]}:{isError:true,content:[{type:'text',text:'failed with image'},{type:'image',mimeType:'image/png',data:'${image}'}]};console.log(JSON.stringify({jsonrpc:'2.0',id:r.id,result}))})`)
  fs.writeFileSync(path.join(f.workspace,'.mcp.json'),JSON.stringify({mcpServers:{image:{command:process.execPath,args:[server]}}}))
  const entry = (await f.request('getLocalExtensions',{workspace:f.workspace})).entries.find(e=>e.kind==='mcp')
  await f.request('setLocalExtensionEnabled',{workspace:f.workspace,id:entry.id,enabled:true})
  const discovery = await f.request('refreshMcp',{workspace:f.workspace})
  assert.equal(discovery.servers[0].error,null)
  fs.writeFileSync(f.script,JSON.stringify({turns:[{tool_calls:[{name:discovery.tools[0],arguments:{}}]},{text:'done'}]}))
  await f.request('setPermissionMode',{mode:'bypass'})
  const r = await f.run()
  const result = r.detail.messages.flatMap(m=>m.content).find(b=>b.type==='tool_result')
  assert.equal(result.is_error,true)
  assert.equal(result.images.length,1)
  assert.equal(r.detail.events.find(e=>e.type==='tool_call_result').data.images[0].asset_id,result.images[0].asset_id)
})

test('S1: session list pagination is optional and validates its bounds', async t => {
  const f = await fixture(t,[{text:'one'},{text:'two'},{text:'three'}])
  for (let i=0;i<3;i++) { await f.request('resetSession'); await f.run({text:`task ${i}`}) }
  const all = await f.request('listSessions')
  assert.equal(all.length,3)
  const page = await f.request('listSessions',{offset:1,limit:1})
  assert.deepEqual(page,[all[1]])
  assert.deepEqual(await f.request('listSessions',{offset:3,limit:1}),[])
  await assert.rejects(f.request('listSessions',{offset:-1,limit:1}),/offset/)
  await assert.rejects(f.request('listSessions',{limit:0}),/limit/)
})
