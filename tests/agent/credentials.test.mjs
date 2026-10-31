import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import http from 'node:http'
import {test} from 'node:test'
import {fixture,connect} from './helpers.mjs'

test('credentials survive restart and blank edits without exposing keys or accepting forged references',async t=>{
  const f=await fixture(t),received=[]
  const server=http.createServer((req,res)=>{received.push(req.headers.authorization);res.setHeader('content-type','application/json');res.end('{"data":[{"id":"fixture"}]}')})
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));t.after(()=>server.close())
  const draft={name:'Credential fixture',baseUrl:`http://127.0.0.1:${server.address().port}/v1`,apiStyle:'openai',apiKey:'initial-placeholder-key',models:[]}
  let saved=await f.request('saveService',{service:draft})
  const file=path.join(f.root,'.minicode','desktop-config.json'),raw=fs.readFileSync(file,'utf8'),stored=JSON.parse(raw).services[0]
  assert.ok(!raw.includes(draft.apiKey));assert.ok(stored.credentialRef);assert.equal(stored.apiKey,undefined)
  assert.equal(saved.services[0].credentialRef,undefined);assert.equal(saved.services[0].apiKey,'')
  await f.close();const next=connect(f)
  await next.request('fetchServiceModels',{service:{...saved.services[0],credentialRef:'forged'}})
  assert.equal(received.at(-1),'Bearer initial-placeholder-key')
  saved=await next.request('saveService',{service:{...saved.services[0],name:'Edited',apiKey:''}})
  assert.equal(saved.services[0].hasApiKey,true)
  await assert.rejects(next.request('fetchServiceModels',{service:{...draft,id:'unknown',apiKey:'',credentialRef:stored.credentialRef}}),/API|密钥/)
  saved=await next.request('saveService',{service:{...saved.services[0],apiKey:'replacement-placeholder-key'}})
  await next.request('fetchServiceModels',{service:saved.services[0]})
  assert.equal(received.at(-1),'Bearer replacement-placeholder-key')
  const final=fs.readFileSync(file,'utf8')
  for(const key of ['initial-placeholder-key','replacement-placeholder-key']){
    assert.ok(!final.includes(key));assert.ok(!JSON.stringify(saved).includes(key));assert.ok(!JSON.stringify(f.events).includes(key));assert.ok(!f.stderr.includes(key));assert.ok(!next.stderr.includes(key))
  }
  if(process.platform==='win32'){
    const latest=JSON.parse(final).services[0]
    fs.writeFileSync(path.join(f.root,'.minicode','desktop-config.credentials',latest.credentialRef),'corrupted')
    await assert.rejects(next.request('getConfiguration'),/credential unavailable/)
  }
})
