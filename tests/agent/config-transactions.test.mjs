import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import {test} from 'node:test'
import {fixture,connect} from './helpers.mjs'

const service=name=>({name,baseUrl:'https://example.test/v1',apiStyle:'openai',apiKey:'transaction-placeholder',enabled:true,models:[]})

test('two bridge processes preserve independent simultaneous configuration changes',async t=>{
  const f=await fixture(t),other=connect(f)
  await Promise.all(Array.from({length:12},(_,i)=>(i%2?f:other).request('saveService',{service:service(`service-${i}`)})))
  const data=await f.request('getConfiguration')
  assert.equal(data.services.length,12)
  assert.equal(new Set(data.services.map(s=>s.name)).size,12)
  const raw=JSON.parse(fs.readFileSync(path.join(f.root,'.minicode','desktop-config.json'),'utf8'))
  assert.equal(raw.version,12,'saveService persists exactly one transaction')
})

test('stale service drafts report a conflict instead of overwriting a newer edit',async t=>{
  const f=await fixture(t),other=connect(f)
  const initial=(await f.request('saveService',{service:service('initial')})).services[0]
  await other.request('saveService',{service:{...initial,name:'newer'}})
  await assert.rejects(f.request('saveService',{service:{...initial,name:'stale'}}),/conflict|冲突/)
  assert.equal((await f.request('getConfiguration')).services[0].name,'newer')
})

test('stale agent editors cannot undo a concurrent disable or resurrect a deleted agent',async t=>{
  const f=await fixture(t),other=connect(f)
  const agents=await f.request('saveAgent',{agent:{name:'transaction-agent',description:'test',instructions:'read files',tools:['read']}})
  const initial=agents.find(a=>a.name==='transaction-agent')
  const draft={...initial,originalName:initial.name,instructions:'edited instructions'}
  await other.request('setAgentEnabled',{name:initial.name,revision:initial.revision,enabled:false})
  await assert.rejects(f.request('saveAgent',{agent:draft}),/conflict/)
  let current=(await f.request('getConfiguration')).agents.find(a=>a.name===initial.name)
  assert.equal(current.enabled,false)
  await other.request('deleteAgent',{name:initial.name,revision:current.revision})
  await assert.rejects(f.request('saveAgent',{agent:{...draft,revision:current.revision}}),/conflict/)
})
