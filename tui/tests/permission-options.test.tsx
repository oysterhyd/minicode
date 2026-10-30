import assert from 'node:assert/strict'
import test from 'node:test'
import React from 'react'
import {render} from 'ink-testing-library'
import {App} from '../src/App.js'
import {delay,FakeBridge} from './fake-bridge.js'

test('R26 scoped approval displays the rule and P saves it to project scope',async()=>{
  const client=new FakeBridge()
  const {stdin,lastFrame,unmount}=render(React.createElement(App,{client,options:{workspace:'D:\\demo',provider:'fake'},version:'1'}))
  try {
    await delay(80);stdin.write('run tests');await delay(20);stdin.write('\r');await delay(40)
    client.emit({event:'approval',sessionId:'sess-1',approvalId:'a',request:{tool_name:'bash',arguments:{command:'npm test'},summary:'npm test',grant_rule:'bash(npm test:*)'}})
    await delay(40)
    const frame=lastFrame()||''
    assert.match(frame,/仅本次/);assert.match(frame,/本会话允许：bash\(npm test:\*\)/);assert.match(frame,/写入项目规则/)
    stdin.write('p');await delay(40)
    assert.ok(client.requests.some(request=>request.method==='resolveApproval'&&request.params.remember==='project'&&request.params.granted===true))
  }finally{unmount()}
})
