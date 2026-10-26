import assert from 'node:assert/strict'
import test from 'node:test'
import React from 'react'
import {render} from 'ink-testing-library'
import {App} from '../src/App.js'
import {BridgeRequestTimeout} from '../src/bridge.js'
import {delay,FakeBridge} from './fake-bridge.js'

test('D3: a send deadline reconciles the accepted run without restoring the draft',async()=>{
 const client=new FakeBridge()
 const state=await client.request<any>('getState')
 client.handlers.set('sendPrompt',()=>{throw new BridgeRequestTimeout('deadline')})
 const {stdin,lastFrame,unmount}=render(React.createElement(App,{client,options:{workspace:'D:/demo',provider:'fake'}}))
 try {
  await delay(100)
  client.handlers.set('getState',()=>({...state,running:true,sessionId:'accepted',runId:'run'}))
  stdin.write('accepted prompt');await delay(30);stdin.write('\r');await delay(100)
  assert.equal((lastFrame()||'').split('accepted prompt').length-1,1)
  client.emit({event:'text_delta',sessionId:'accepted',runId:'run',text:'still running'})
  await delay(80);assert.match(lastFrame()||'',/still running/)
 }finally{unmount()}
})
