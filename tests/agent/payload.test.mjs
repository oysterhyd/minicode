import assert from 'node:assert/strict'
import {test} from 'node:test'
import {fixture} from './helpers.mjs'
import {sseMock,anthropicFrames,configureService} from './sse-mock.mjs'
function assertStrictRoles(messages){
  assert.equal(messages[0].role,'user')
  for(let i=1;i<messages.length;i++)assert.notEqual(messages[i].role,messages[i-1].role,JSON.stringify(messages.map(m=>m.role)))
  for(const m of messages.filter(m=>m.role==='user')){const types=m.content.map(b=>b.type),first=types.findIndex(t=>t!=='tool_result');assert.ok(first<0||!types.slice(first).includes('tool_result'),JSON.stringify(types))}
}
test('Anthropic wire payload stays strictly alternating after a background completion notice',async t=>{
  const f=await fixture(t);const tools=[{id:'bg',name:'bash',arguments:{command:'node -e "console.log(42)"',background:true}},{id:'fg',name:'bash',arguments:{command:'node -e "setTimeout(()=>{},1500)"'}}]
  const mock=await sseMock(t,[{frames:anthropicFrames({tools})},{frames:anthropicFrames({text:'done'})}])
  await f.request('setPermissionMode',{mode:'bypass'});const r=await f.run({model:await configureService(f,mock.url)})
  assert.equal(r.result.exit_reason,'completed');assert.equal(mock.payloads.length,2)
  assert.deepEqual(r.detail.messages.map(m=>m.role),['user','assistant','user','user','assistant'])
  const sent=mock.payloads[1].messages;assertStrictRoles(sent);assert.equal(sent.length,3);assert.deepEqual(sent[2].content.map(b=>b.type),['tool_result','tool_result','text'])
})
