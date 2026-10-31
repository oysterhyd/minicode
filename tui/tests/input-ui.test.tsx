import assert from 'node:assert/strict'
import test from 'node:test'
import React from 'react'
import {render} from 'ink-testing-library'
import {App} from '../src/App.js'
import {delay,FakeBridge} from './fake-bridge.js'

test('T-UI6 10000-line bracketed paste renders a reference and sends losslessly', async () => {
 const client=new FakeBridge(),text='paste 中文\n'.repeat(10000)
 const ui=render(<App client={client} options={{workspace:'/repo',provider:'fake'}} />)
 try {
  await delay(100);ui.stdin.write('\x1b[200~');ui.stdin.write(text);ui.stdin.write('\x1b[201~');await delay(40)
  assert.match(ui.lastFrame()||'',/\[paste:1\]/)
  assert.ok((ui.lastFrame()||'').length<10000)
  ui.stdin.write('\r');await delay(50)
  assert.equal(client.requests.find(r=>r.method==='sendPrompt')?.params.text,text)
 } finally {ui.unmount()}
})
test('T-UI6 image import paths preserve spaces and attach to the next prompt', async () => {
 const client=new FakeBridge();client.handlers.set('importAttachments',()=>[{asset_id:'image-id',name:'image name.png'}])
 const ui=render(<App client={client} options={{workspace:'/repo',provider:'fake'}} />)
 try {
  await delay(100);ui.stdin.write('/attach "image name.png"');ui.stdin.write('\r');await delay(40)
  ui.stdin.write('inspect image');ui.stdin.write('\r');await delay(50)
  assert.deepEqual(client.requests.find(r=>r.method==='sendPrompt')?.params.attachments,[{asset_id:'image-id',name:'image name.png'}])
 } finally {ui.unmount()}
})
