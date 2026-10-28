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

test('shell disables credential, editor and pager prompts by default', async t => {
  const names = ['GIT_TERMINAL_PROMPT','GIT_EDITOR','GIT_SEQUENCE_EDITOR','PAGER','GIT_PAGER','CI']
  const env = Object.fromEntries(names.map(name => [name, '']))
  const command = `node -e "console.log([${names.map(name => `process.env.${name}`).join(',')}].join(','))"`
  const f = await fixture(t, [{tool_calls:[{name:'bash',arguments:{command}}]}, {text:'done'}], env)
  await f.request('setPermissionMode', {mode:'bypass'})
  const result = await f.run()
  assert.equal(result.detail.events.find(e => e.type === 'tool_call_result').data.output_detail.trim(), '0,true,true,cat,cat,1')
})
