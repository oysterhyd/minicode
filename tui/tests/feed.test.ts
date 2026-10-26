import assert from 'node:assert/strict'
import test from 'node:test'
import { acceptsRunEvent, applyDesktopEvent, bindFeedSession, emptyFeed, isLive, pendingApproval } from '../src/feed.js'

test('TU1: abandoned sessions stay rejected during a new send and can be explicitly resumed', () => {
  let state = bindFeedSession(emptyFeed(), 'old')
  state = bindFeedSession(state, null)
  state = { ...state, busy: true, awaitingRun: true }
  const late = { event: 'run_done' as const, sessionId: 'old', result: { exit_reason: 'completed' } }
  assert.equal(acceptsRunEvent(state, late), false)
  assert.equal(applyDesktopEvent(state, late), state)
  assert.equal(acceptsRunEvent(state, { event: 'run_started', sessionId: 'new', runId: 'new-run' }), true)
  state = bindFeedSession(state, 'old')
  assert.equal(acceptsRunEvent(state, { event: 'text_delta', sessionId: 'old', text: 'resumed' }), true)
})

test('MCP startup diagnostics stay out of chat while tool failures remain visible', () => {
  let state = emptyFeed()
  state = applyDesktopEvent(state, { event: 'agent_event', item: { seq: 1, type: 'mcp_discovery', data: { server: 'offline', error: 'MCP connection failed' } } })
  assert.equal(state.items.length, 0)
  state = applyDesktopEvent(state, { event: 'agent_event', item: { seq: 2, type: 'tool_call_start', data: { call_id: 'mcp-call', name: 'mcp__online__echo', arguments: {} } } })
  state = applyDesktopEvent(state, { event: 'agent_event', item: { seq: 3, type: 'tool_call_result', data: { call_id: 'mcp-call', success: false, output_detail: 'connection lost', error: 'connection lost' } } })
  assert.equal(state.items.length, 1)
  assert.equal(state.items[0].success, false)
  assert.match(state.items[0].output || '', /connection lost/)
})

test('single-row updates preserve old state and completed row identities', () => {
  let state = emptyFeed()
  state.items.push({ id: 'history', kind: 'assistant', text: 'completed' })
  state = applyDesktopEvent(state, { event: 'text_delta', text: 'stream' })
  const next = applyDesktopEvent(state, { event: 'text_delta', text: ' appended' })
  assert.notEqual(next.items, state.items)
  assert.equal(next.items[0], state.items[0])
  assert.equal(state.items[1].text, 'stream')
  assert.equal(next.items[1].text, 'stream appended')
})

test('retry clears only the current streamed answer', () => {
  let state = emptyFeed()
  state.items.push({ id: 'history', kind: 'assistant', text: 'older answer' })
  state = applyDesktopEvent(state, { event: 'text_delta', text: 'discard partial' })
  state = applyDesktopEvent(state, { event: 'agent_event', item: { seq: 4, type: 'provider_retry', data: { reset_output: true, delay_s: 0, next_attempt: 2 } } })
  state = applyDesktopEvent(state, { event: 'text_delta', text: 'new answer' })
  assert.ok(state.items.some(item => item.text === 'older answer'))
  assert.ok(state.items.some(item => item.text === 'new answer'))
  assert.ok(!state.items.some(item => item.text?.includes('discard partial')))
})

test('text delta then assistant message becomes one reply', () => {
  let state = emptyFeed()
  state = applyDesktopEvent(state, { event: 'text_delta', text: '我会先' })
  state = applyDesktopEvent(state, { event: 'text_delta', text: '核对分页。' })
  state = applyDesktopEvent(state, {
    event: 'agent_event',
    item: { seq: 1, type: 'assistant_message', timestamp: '2026-01-01T00:00:00Z', data: { text: '我会先核对分页。' } },
  })
  assert.equal(state.items.length, 1)
  assert.equal(state.items[0].kind, 'assistant')
  assert.equal(state.items[0].text, '我会先核对分页。')
  assert.equal(state.items[0].pending, undefined)
})

test('tool start and result stay one line', () => {
  let state = emptyFeed()
  state = applyDesktopEvent(state, {
    event: 'agent_event',
    item: {
      seq: 2,
      type: 'tool_call_start',
      timestamp: '2026-01-01T00:00:00Z',
      data: { call_id: 'c1', name: 'read', arguments: { path: 'paginate.py' } },
    },
  })
  assert.equal(state.items[0].pending, true)
  state = applyDesktopEvent(state, {
    event: 'agent_event',
    item: {
      seq: 3,
      type: 'tool_call_result',
      timestamp: '2026-01-01T00:00:01Z',
      data: { call_id: 'c1', success: true, output_preview: 'ok' },
    },
  })
  assert.equal(state.items[0].pending, false)
  assert.equal(state.items[0].success, true)
  assert.equal(state.items[0].output, 'ok')
})

test('approval sits in the feed until decided', () => {
  let state = emptyFeed()
  state = applyDesktopEvent(state, {
    event: 'approval',
    approvalId: 'a1',
    request: { tool_name: 'write', arguments: { path: 'new.txt' }, summary: 'write new.txt' },
  })
  assert.equal(pendingApproval(state.items)?.approvalId, 'a1')
  state = applyDesktopEvent(state, {
    event: 'agent_event',
    item: { seq: 4, type: 'approval_decision', timestamp: '2026-01-01T00:00:02Z', data: { granted: true } },
  })
  assert.equal(pendingApproval(state.items), undefined)
  assert.equal(state.items[0].granted, true)
  assert.equal(isLive(state.items[0]), false)
})

test('streaming assistant and pending tools stay live', () => {
  let state = emptyFeed()
  state = applyDesktopEvent(state, { event: 'text_delta', text: '流式' })
  assert.equal(isLive(state.items[0]), true)
  state = applyDesktopEvent(state, {
    event: 'agent_event',
    item: {
      seq: 5,
      type: 'tool_call_start',
      timestamp: '2026-01-01T00:00:03Z',
      data: { call_id: 'c2', name: 'read', arguments: { path: 'a.py' } },
    },
  })
  assert.equal(isLive(state.items.at(-1)!), true)
})

// Protocol warnings must not end the run or populate its error channel.
test('protocol warnings are notices and log events do not change feed state', () => {
  const state = {...emptyFeed(),busy:true,sessionId:'active'}
  const logged = applyDesktopEvent(state,{event:'log',text:'diagnostic'})
  assert.equal(logged,state)
  const warned = applyDesktopEvent(state,{event:'warning',sessionId:'active',text:'optional feature unavailable'})
  assert.equal(warned.busy,true)
  assert.equal(warned.error,'')
  assert.equal(warned.items[0].tone,'warning')
})

test('B20: stale terminal events do not finish the current run, legacy events still work', () => {
  const state={...emptyFeed(),busy:true,sessionId:'session',runId:'new'}
  for (const event of ['run_done','run_error'] as const) {
    assert.equal(applyDesktopEvent(state,{event,sessionId:'session',runId:'old',error:'old failure'}),state)
  }
  assert.equal(applyDesktopEvent(state,{event:'run_done',sessionId:'session',runId:'new'}).busy,false)
  assert.equal(applyDesktopEvent(state,{event:'run_done',sessionId:'session'}).busy,false)
})

test('B20: a terminal arriving while the next send is awaiting its identity stays ignored', () => {
  let state={...emptyFeed(),busy:true,awaitingRun:true,sessionId:'session',runId:'old'}
  assert.equal(applyDesktopEvent(state,{event:'run_done',sessionId:'session',runId:'old'}),state)
  state=applyDesktopEvent(state,{event:'run_started',sessionId:'session',runId:'new'})
  assert.equal(state.runId,'new')
  assert.equal(state.busy,true)
  assert.equal(applyDesktopEvent(state,{event:'run_done',sessionId:'session',runId:'old'}),state)
  assert.equal(applyDesktopEvent(state,{event:'run_done',sessionId:'session',runId:'new'}).busy,false)
})

test('B22: a loaded interrupted tool is repaired when its live result arrives',()=>{
 const state={...emptyFeed(),items:[{id:'call',kind:'tool' as const,name:'read',interrupted:true}]}
 const next=applyDesktopEvent(state,{event:'agent_event',item:{seq:2,type:'tool_call_result',timestamp:new Date().toISOString(),data:{call_id:'call',success:true,output_preview:'result'}}})
 assert.equal(next.items[0].interrupted,false)
 assert.equal(next.items[0].success,true)
})
