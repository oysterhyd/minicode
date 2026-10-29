import assert from 'node:assert/strict'
import {test} from 'node:test'
import {fixture} from './helpers.mjs'
import {sseMock,anthropicFrames,configureService} from './sse-mock.mjs'

const summary = next => `## Goal
完成任务
## Constraints & Preferences
保留接口
## Progress
### Done
已修改 agent/context.rs，cargo test 通过
### In Progress
文档
### Blocked
无
## Key Decisions
选择兼容方案
## Next Steps
${next}
## Critical Context
### Read Files
agent/tests.rs
### Modified Files
agent/context.rs`

test('X1: structured summary requests are tool-free and update the previous summary', async t => {
  const f = await fixture(t)
  let summaries = 0
  const mock = await sseMock(t, payload => {
    if (JSON.stringify(payload.system).includes('Summarize the conversation data')) {
      assert.equal((payload.tools ?? []).length, 0)
      if (summaries > 0) assert.match(JSON.stringify(payload.messages), /first-next-step/)
      return {frames:anthropicFrames({text:summary(++summaries === 1 ? 'first-next-step' : 'second-next-step'),input:50,output:20})}
    }
    return {frames:anthropicFrames({text:'已改 context.rs；关键决策为保持接口；测试通过，下一步提交。'.repeat(300)})}
  })
  const model = await configureService(f, mock.url)
  const first = await f.run({model,text:'original constraints'})
  await f.run({model,text:'older task',sessionId:first.id})
  await f.run({model,text:'latest task',sessionId:first.id})
  const compact = () => f.request('runSlash',{workspace:f.workspace,sessionId:first.id,text:'/compact'})
  assert.match((await compact()).message, /已压缩/)
  await f.run({model,text:'next task',sessionId:first.id})
  assert.match((await compact()).message, /已压缩/)
  assert.equal(summaries,2)
  const detail = await f.request('getSession',{sessionId:first.id})
  const events = detail.events.filter(e => e.type === 'context_compacted')
  assert.ok(events.every(e => e.data.summarized_units > 0))
  assert.ok(detail.events.filter(e => e.type === 'context_summary').every(e => e.data.usage.input_tokens === 50))
})

test('X3: /compact handles a single long reply and explicitly reports no further work', async t => {
  const f = await fixture(t,[{text:'已完成修改，下一步验证。'.repeat(4000)}])
  const first = await f.run({text:'原始任务要求必须保留'})
  const compact = () => f.request('runSlash',{workspace:f.workspace,sessionId:first.id,text:'/compact'})
  assert.match((await compact()).message,/已压缩/)
  assert.match((await compact()).message,/没有可压缩/)
})


test('X4: context_length_exceeded compacts history and retries the same round', async t => {
  const f = await fixture(t)
  let requests = 0
  const mock = await sseMock(t, payload => {
    if (JSON.stringify(payload.system).includes('Summarize the conversation data')) return {status:503,body:'summary unavailable'}
    requests++
    if (requests === 1) return {frames:anthropicFrames({text:'此前进度与决定。'.repeat(3000)})}
    if (requests === 2) return {status:400,body:JSON.stringify({error:{code:'context_length_exceeded',message:'prompt too long'}})}
    return {frames:anthropicFrames({text:'recovered'})}
  })
  const model = await configureService(f,mock.url)
  const first = await f.run({model,text:'keep original constraints'})
  const result = await f.run({model,sessionId:first.id,text:'continue'})
  assert.equal(result.result.exit_reason,'completed')
  assert.equal(requests,3)
  const retry = result.events.filter(e => e.event === 'agent_event' && e.item.type === 'provider_retry')
  assert.equal(retry.length,1)
  assert.equal(retry[0].item.data.reason,'context_overflow')
  const taskPayloads = mock.payloads.filter(p => !JSON.stringify(p.system).includes('Summarize the conversation data'))
  assert.ok(JSON.stringify(taskPayloads[2].messages).length < JSON.stringify(taskPayloads[1].messages).length / 2)
  assert.match(JSON.stringify(taskPayloads[2].messages),/keep original constraints/)
})
