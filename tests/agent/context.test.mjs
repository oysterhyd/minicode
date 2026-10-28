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
