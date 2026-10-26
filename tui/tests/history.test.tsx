import assert from 'node:assert/strict'
import test from 'node:test'
import React from 'react'
import {render} from 'ink-testing-library'
import {App} from '../src/App.js'
import {delay, FakeBridge} from './fake-bridge.js'

test('B14: resume loads one page and /history fetches earlier history on demand', async () => {
  const client = new FakeBridge()
  const state = await client.request<any>('getState')
  client.handlers.set('runSlash', () => ({action:'resume',sessionId:'old'}))
  client.handlers.set('selectSession', () => ({...state,sessionId:'old'}))
  client.handlers.set('getSession', () => ({summary:{session_id:'old',workspace:'/repo',model:'fake'},messages:[{role:'user',content:[{type:'text',text:'recent history'}]}],events:[],messageOffset:1,cursor:{messages:1,events:null}}))
  client.handlers.set('getSessionPage', () => ({summary:{session_id:'old',workspace:'/repo',model:'fake'},messages:[{role:'user',content:[{type:'text',text:'earliest history'}]}],events:[],messageOffset:0,cursor:{messages:null,events:null}}))
  const {stdin,lastFrame,unmount} = render(React.createElement(App,{client,options:{workspace:'/repo',provider:'fake'}}))
  try {
    await delay(250); stdin.write('/resume old'); await delay(80); stdin.write('\r'); await delay(250)
    assert.equal(client.requests.filter(r=>r.method==='getSession').length,1,JSON.stringify(client.requests) + lastFrame())
    assert.equal(client.requests.filter(r=>r.method==='getSessionPage').length,0)
    stdin.write('/history'); await delay(80); stdin.write('\r'); await delay(250)
    assert.deepEqual(client.requests.filter(r=>r.method==='getSessionPage').map(r=>r.params.cursor),[{messages:1,events:null}])
  } finally { unmount() }
})

test('B14: a late history page preserves live updates to already loaded tools', async () => {
  const client = new FakeBridge()
  const state = await client.request<any>('getState')
  client.handlers.set('runSlash', () => ({action:'resume',sessionId:'old'}))
  client.handlers.set('selectSession', () => ({...state,sessionId:'old'}))
  client.handlers.set('getSession', () => ({summary:{session_id:'old',workspace:'/repo',model:'fake'},messages:[{role:'assistant',content:[{type:'tool_use',id:'call',name:'bash',input:{command:'echo live'}}]}],events:[{seq:5,type:'tool_call_result',timestamp:new Date().toISOString(),data:{call_id:'call',success:true,output_preview:'snapshot output'}}],messageOffset:1,cursor:{messages:1,events:null}}))
  let release!: (value: unknown) => void
  client.handlers.set('getSessionPage', () => new Promise(resolve => { release=resolve }))
  const {stdin,lastFrame,unmount} = render(React.createElement(App,{client,options:{workspace:'/repo',provider:'fake'}}))
  try {
    await delay(250); stdin.write('/resume old'); await delay(80); stdin.write('\r'); await delay(250)
    stdin.write('/history'); await delay(80); stdin.write('\r'); await delay(80)
    client.emit({event:'agent_event',sessionId:'old',item:{seq:100,type:'tool_call_result',timestamp:new Date().toISOString(),data:{call_id:'call',success:true,output_preview:'LIVE RESULT'}}})
    release({summary:{session_id:'old',workspace:'/repo',model:'fake'},messages:[{role:'user',content:[{type:'text',text:'earliest history'}]}],events:[],messageOffset:0,cursor:{messages:null,events:null}})
    await delay(100); stdin.write('\x0f'); await delay(100)
    assert.match(lastFrame() || '',/LIVE RESULT/)
  } finally {unmount()}
})
