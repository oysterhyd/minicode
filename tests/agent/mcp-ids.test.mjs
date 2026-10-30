import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import http from 'node:http'
import {test} from 'node:test'
import {fixture} from './helpers.mjs'

async function enable(f,config) {
  fs.writeFileSync(path.join(f.workspace,'.mcp.json'),JSON.stringify({mcpServers:{ids:{...config,timeout_s:1}}}))
  const entry=(await f.request('getLocalExtensions',{workspace:f.workspace})).entries.find(e=>e.kind==='mcp')
  await f.request('setLocalExtensionEnabled',{workspace:f.workspace,id:entry.id,enabled:true})
  fs.writeFileSync(f.script,JSON.stringify({turns:[{tool_calls:[{name:`mcp__${entry.runtimeName}__echo`,arguments:{}}]},{text:'done'}]}))
  await f.request('setPermissionMode',{mode:'bypass'})
}
function result(r) {
  return r.method==='initialize'?{protocolVersion:'2025-11-25',serverInfo:{name:'ids',version:'1'}}:
    r.method==='tools/list'?{tools:[{name:'echo',inputSchema:{type:'object'}}]}:{content:[{type:'text',text:'correct'}]}
}
function encode(r,variant) {
  const id=variant==='string'?String(r.id):r.id
  const json=JSON.stringify({jsonrpc:'2.0',id,result:result(r)})
  return variant==='float'?json.replace(`"id":${r.id},`,`"id":${r.id}.0,`):json
}
function checked(run) {
  assert.equal(run.detail.events.find(e=>e.type==='mcp_discovery').data.error,null)
  const output=run.detail.events.find(e=>e.type==='tool_call_result').data
  assert.equal(output.success,true,output.error);assert.equal(output.output_detail,'correct')
}

for(const variant of ['integer','string','float'])test(`MC5 stdio accepts ${variant} response IDs`,async t=>{
  const f=await fixture(t),file=path.join(f.root,'server.cjs')
  fs.writeFileSync(file,`const result=${result.toString()};const encode=${encode.toString()};require('node:readline').createInterface({input:process.stdin}).on('line',line=>{const r=JSON.parse(line);if(r.id!==undefined)console.log(encode(r,${JSON.stringify(variant)}))});`)
  await enable(f,{command:process.execPath,args:[file]});checked(await f.run())
})

for(const transport of ['json','sse','legacy'])for(const variant of ['string','float'])test(`MC5 ${transport} accepts ${variant} response IDs`,async t=>{
  const f=await fixture(t);let events
  const server=http.createServer((req,res)=>{
    if(req.method==='GET') {events=res;res.writeHead(200,{'content-type':'text/event-stream'});res.write('event: endpoint\ndata: /messages\n\n');return}
    let body='';req.on('data',b=>body+=b);req.on('end',()=>{
      const r=JSON.parse(body)
      if(r.id===undefined){res.writeHead(202);res.end();return}
      const value=encode(r,variant)
      if(transport==='legacy'){res.writeHead(202);res.end();events.write('data: '+value+'\n\n')}
      else if(transport==='sse'){res.writeHead(200,{'content-type':'text/event-stream'});res.end('data: '+value+'\n\n')}
      else {res.setHeader('content-type','application/json');res.end(value)}
    })
  })
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve))
  t.after(()=>{server.closeAllConnections();return new Promise(resolve=>server.close(resolve))})
  await enable(f,{url:`http://127.0.0.1:${server.address().port}/mcp`,transport:transport==='legacy'?'sse':'http'})
  checked(await f.run())
})

test('MC5 invalid and rounded fractional IDs cannot satisfy a different request',async t=>{
  const f=await fixture(t),file=path.join(f.root,'server.cjs')
  fs.writeFileSync(file,`const result=${result.toString()};require('node:readline').createInterface({input:process.stdin}).on('line',line=>{
    const r=JSON.parse(line);if(r.id===undefined)return;
    for(const id of ['true','null','{}','[]','"0'+r.id+'"',r.id+'.0000000000000001'])process.stdout.write('{"jsonrpc":"2.0","id":'+id+',"result":{"content":[{"type":"text","text":"poison"}]}}\\n');
    console.log(JSON.stringify({jsonrpc:'2.0',id:r.id,result:result(r)}));
  });`)
  await enable(f,{command:process.execPath,args:[file]});checked(await f.run())
})
