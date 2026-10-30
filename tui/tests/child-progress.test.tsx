import assert from 'node:assert/strict'
import test from 'node:test'
import React from 'react'
import { render } from 'ink-testing-library'
import { App } from '../src/App.js'
import { applyDesktopEvent, emptyFeed } from '../src/feed.js'
import { historyItems, type AgentEvent } from '../src/types.js'
import { delay, FakeBridge } from './fake-bridge.js'

const event=(seq:number,type:string,data:Record<string,unknown>):AgentEvent=>({seq,type,data,timestamp:'2026-01-01T00:00:00Z'})
const start=event(1,'tool_call_start',{call_id:'d',name:'delegate',arguments:{kind:'explore',task:'same'}})
const child=event(3,'subagent_start',{call_id:'d',child_session_id:'child',kind:'explore',task:'same'})
const progress=(seq:number,status:string,childSeq:number)=>event(seq,'subagent_progress',{call_id:'d',child_session_id:'child',child_call_id:'r',name:'read',phase:'tool',status,child_event_seq:childSeq})

test('R33 live and history associate identical tasks by call ID, deduplicate and retain final status',()=>{
  const events=[start,event(2,'tool_call_start',{...start.data,call_id:'other'}),child,progress(4,'completed',8),progress(5,'running',6),progress(6,'completed',8),event(7,'subagent_result',{call_id:'d',child_session_id:'child',exit_reason:'completed'}),progress(8,'running',7)]
  let state=emptyFeed()
  for(const item of events) state=applyDesktopEvent(state,{event:'agent_event',item})
  assert.equal(state.items[0].childProgress?.length,1)
  assert.equal(state.items[0].childProgress?.[0].status,'completed')
  assert.equal(state.items[0].childStatus,'completed')
  assert.equal(state.items[1].childSessionId,undefined)
  const history=historyItems({summary:{session_id:'p',workspace:'.',model:'fake'},messages:[{role:'assistant',content:[{type:'tool_use',id:'d',name:'delegate',input:start.data.arguments},{type:'tool_use',id:'other',name:'delegate',input:start.data.arguments}]}],events})
  assert.deepEqual(history[0].childProgress,state.items[0].childProgress)
  assert.equal(history[0].childStatus,'completed');assert.equal(history[1].childSessionId,undefined)
})

test('R33 Ctrl+O folds child tool progress while delegate remains running',async()=>{
  const client=new FakeBridge()
  const {stdin,lastFrame,unmount}=render(React.createElement(App,{client,options:{workspace:'D:\\demo',provider:'fake'},version:'1'}))
  try {
    await delay(80);stdin.write('inspect');await delay(20);stdin.write('\r');await delay(40)
    for(const item of [start,child,progress(4,'running',6)]) client.emit({event:'agent_event',sessionId:'sess-1',item})
    await delay(40)
    assert.match(lastFrame()||'',/子任务：.*read/)
    assert.doesNotMatch(lastFrame()||'',/read · 运行中/)
    stdin.write('\x0f');await delay(40)
    assert.match(lastFrame()||'',/read · 运行中/)
    stdin.write('\x0f');await delay(40)
    assert.doesNotMatch(lastFrame()||'',/read · 运行中/)
  } finally {unmount()}
})
