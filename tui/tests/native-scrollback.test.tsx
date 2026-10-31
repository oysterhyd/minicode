import assert from 'node:assert/strict'
import test from 'node:test'
import React from 'react'
import {PassThrough,Writable} from 'node:stream'
import {render} from 'ink'
import {App} from '../src/App.js'
import {delay,FakeBridge} from './fake-bridge.js'

test('T-UI9 native scrollback redraw never erases the terminal history buffer', async () => {
 const writes:string[]=[],client=new FakeBridge()
 const output=Object.assign(new Writable({write(chunk,_encoding,done){writes.push(String(chunk));done()}}),{isTTY:true,columns:80,rows:24})
 const input=Object.assign(new PassThrough(),{isTTY:true,setRawMode(){return this},ref(){return this},unref(){return this}})
 const instance=render(<App client={client} options={{workspace:'/repo',provider:'fake'}} scrollback />,{stdout:output as unknown as NodeJS.WriteStream,stdin:input as unknown as NodeJS.ReadStream,stderr:output as unknown as NodeJS.WriteStream,exitOnCtrlC:false,patchConsole:false,maxFps:1000})
 try {
  await delay(100)
  client.emit({event:'run_started',sessionId:'s',runId:'r'})
  client.emit({event:'text_delta',sessionId:'s',runId:'r',text:'streamed output'})
  await delay(30)
  client.emit({event:'agent_event',sessionId:'s',runId:'r',item:{type:'assistant_message',seq:1,timestamp:'',data:{text:'completed output'}}})
  await delay(40)
  client.emit({event:'reasoning_delta',sessionId:'s',runId:'r',text:'long reasoning '.repeat(1000)})
  await delay(20);input.write('\x12');await delay(40)
  assert.ok(writes.join('').includes('completed output'))
  assert.ok(!writes.join('').includes('\x1b[3J'),'Ink fullscreen replay erased native scrollback')
 } finally {instance.unmount();instance.cleanup();input.destroy();output.destroy()}
})
