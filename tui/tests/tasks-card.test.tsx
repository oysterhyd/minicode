import assert from 'node:assert/strict'
import test from 'node:test'
import React from 'react'
import { render } from 'ink-testing-library'
import { App } from '../src/App.js'
import { delay, FakeBridge } from './fake-bridge.js'
test('TU6 task results refresh the persistent task card', async () => {
 const client = new FakeBridge()
 client.handlers.set('listTasks', () => [{task_id:'a',title:'Implement feature',status:'running'},{task_id:'b',title:'Verify feature',status:'done'}])
 const view=render(<App client={client} options={{workspace:'/repo',provider:'fake'}} />)
 try {
  await delay(100);view.stdin.write('go');view.stdin.write('\r');await delay(40)
  client.emit({event:'agent_event',sessionId:'sess-1',item:{seq:2,type:'tool_call_result',timestamp:'',data:{name:'task_claim',success:true}}})
  await delay(100)
  assert.match(view.lastFrame()||'',/Implement feature/)
  assert.match(view.lastFrame()||'',/1\/2/)
 } finally {view.unmount()}
})
