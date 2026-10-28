import assert from 'node:assert/strict'
import {test} from 'node:test'
import {fixture} from './helpers.mjs'

test('shell defaults disable color and explicit preferences survive', async t => {
  for (const explicit of [false, true]) {
    const env = explicit
      ? {NO_COLOR:'custom', TERM:'xterm', CARGO_TERM_COLOR:'always'}
      : {NO_COLOR:'', TERM:'', CARGO_TERM_COLOR:''}
    const command = 'node -e "console.log([process.env.NO_COLOR,process.env.TERM,process.env.CARGO_TERM_COLOR].join(\',\'))"'
    const f = await fixture(t, [{tool_calls:[{name:'bash',arguments:{command}}]}, {text:'done'}], env)
    await f.request('setPermissionMode', {mode:'bypass'})
    const result = await f.run()
    assert.equal(result.detail.events.find(e => e.type === 'tool_call_result').data.output_detail.trim(), explicit ? 'custom,xterm,always' : '1,dumb,never')
  }
})

test('cancelled command persists captured output in its result and history', async t => {
  const command = 'node -e "console.log(\'partial-before-cancel\');setInterval(()=>{},1000)"'
  const f = await fixture(t, [{tool_calls:[{name:'bash',arguments:{command}}]}, {text:'done'}])
  await f.request('setPermissionMode', {mode:'bypass'})
  const started = await f.request('sendPrompt', {text:'run', workspace:f.workspace, model:'fake'})
  await f.wait(e => e.event === 'agent_event' && e.item.type === 'tool_output' && e.item.data.output_preview.includes('partial-before-cancel'))
  await f.request('cancelTurn', {sessionId:started.sessionId})
  await f.wait(e => e.event === 'run_done' && e.sessionId === started.sessionId)
  const detail = await f.request('getSession', {sessionId:started.sessionId})
  const result = detail.events.find(e => e.type === 'tool_call_result')
  assert.ok(result, 'cancelled tool result must be persisted')
  assert.equal(result.data.success, false)
  assert.match(result.data.output_detail, /partial-before-cancel/)
  assert.match(JSON.stringify(detail.messages), /partial-before-cancel/)
})

test('shell disables credential, editor and pager prompts by default', async t => {
  const names = ['GIT_TERMINAL_PROMPT','GIT_EDITOR','GIT_SEQUENCE_EDITOR','PAGER','GIT_PAGER','CI']
  const env = Object.fromEntries(names.map(name => [name, '']))
  const command = `node -e "console.log([${names.map(name => `process.env.${name}`).join(',')}].join(','))"`
  const f = await fixture(t, [{tool_calls:[{name:'bash',arguments:{command}}]}, {text:'done'}], env)
  await f.request('setPermissionMode', {mode:'bypass'})
  const result = await f.run()
  assert.equal(result.detail.events.find(e => e.type === 'tool_call_result').data.output_detail.trim(), '0,true,true,cat,cat,1')
})
