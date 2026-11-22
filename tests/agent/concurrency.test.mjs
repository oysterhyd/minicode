import assert from 'node:assert/strict'
import fs from 'node:fs'
import http from 'node:http'
import path from 'node:path'
import {test} from 'node:test'
import {setTimeout as delay} from 'node:timers/promises'
import {fixture} from './helpers.mjs'
import {anthropicFrames, configureService} from './sse-mock.mjs'

const minicodeVersion=JSON.parse(fs.readFileSync(new URL('../../release/package.json',import.meta.url),'utf8')).version

test('B5/B15: concurrent catalog scans cannot delay another sessions cancellation by 100ms', async t => {
  const f = await fixture(t,[{tool_calls:[{name:'write',arguments:{path:'needs-approval.txt',content:'pending'}}]}])
  const started = await f.request('sendPrompt',{clientKey:'active',workspace:f.workspace,model:'fake',text:'wait for approval'})
  await f.wait(e=>e.event==='approval')
  const workspace=path.join(f.root,'slow-repo'),plugin=path.join(workspace,'.minicode/plugins/dense')
  fs.mkdirSync(plugin,{recursive:true})
  fs.writeFileSync(path.join(plugin,'plugin.json'),JSON.stringify({name:'dense',version:'1.0.0',minicode_version:minicodeVersion,enabled:true}))
  const content=Buffer.alloc(30_000,'x')
  for(let i=0;i<500;i++)fs.writeFileSync(path.join(plugin,`${i}.txt`),content)
  let completed=0
  const scans=['slow-a','slow-b'].map(clientKey=>f.request('getCapabilities',{workspace,clientKey}).then(()=>{completed++}))
  t.after(()=>Promise.all(scans))
  await delay(20)
  assert.equal(completed,0,'scan fixture did not overlap cancellation')
  const start=performance.now()
  assert.equal(await f.request('cancelTurn',{sessionId:started.sessionId}),true)
  const ms=performance.now()-start
  t.diagnostic(`cancel acknowledgement: ${ms.toFixed(1)}ms`)
  assert.ok(ms<100,`cancellation waited ${ms.toFixed(1)}ms for catalog scans`)
  await Promise.all(scans)
  assert.equal((await f.wait(e=>e.event==='run_done'&&e.sessionId===started.sessionId)).result.exit_reason,'cancelled')
})

test('read-only delegate calls run up to 4 concurrently per wave and isolate duplicate keys', async t => {
  const f = await fixture(t)
  let parentRound = 0, inFlight = 0, maxInFlight = 0, wave1Arrived = 0, wave2SawOverlap = false
  let releaseWave1
  const wave1Barrier = new Promise(resolve => { releaseWave1 = resolve })
  const sockets = new Set()
  const server = http.createServer((req, res) => {
    let body = ''
    req.on('data', d => { body += d })
    req.on('end', async () => {
      const payload = JSON.parse(body)
      const isChild = JSON.stringify(payload.system).includes('你是子助手。')
      let frames
      if (!isChild) {
        parentRound++
        frames = parentRound === 1
          ? anthropicFrames({
              tools: [1, 2, 3, 4, 5].map(i => ({
                id: `d${i}`,
                name: 'delegate',
                arguments: {kind: 'explore', task: `task-${i}`},
              })),
              input: 100,
              output: 20,
            })
          : anthropicFrames({text: 'all 5 explored', input: 120, output: 15})
      } else {
        const task = JSON.stringify(payload.messages).match(/task-[1-5]/)?.[0] || 'unknown'
        inFlight++
        if (inFlight > maxInFlight) maxInFlight = inFlight
        if (task !== 'task-5') {
          wave1Arrived++
          if (wave1Arrived === 4) releaseWave1()
          await Promise.race([wave1Barrier, delay(3000)])
        } else {
          if (inFlight > 1) wave2SawOverlap = true
          await delay(20)
        }
        frames = anthropicFrames({
          text: JSON.stringify({summary: `done ${task}`, findings: [], evidence_refs: [], unresolved: []}),
          input: 50,
          output: 10,
        })
        inFlight--
      }
      res.writeHead(200, {'content-type': 'text/event-stream'})
      for (const frame of frames) res.write(`data: ${JSON.stringify(frame)}\n\n`)
      res.end()
    })
  })
  server.on('connection', socket => { sockets.add(socket); socket.on('close', () => sockets.delete(socket)) })
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
  t.after(() => { for (const s of sockets) s.destroy(); return new Promise(resolve => server.close(resolve)) })

  const model = await configureService(f, `http://127.0.0.1:${server.address().port}`)
  const run = await f.run({model, text: 'explore 5 modules in parallel'})
  assert.equal(run.result.exit_reason, 'completed')
  assert.equal(maxInFlight, 4, 'first wave of 4 distinct read-only delegates must run concurrently, capped at 4')
  assert.equal(wave2SawOverlap, false, '5th delegate must wait for the 4-wide batch to finish')
  assert.equal(run.result.total_usage.input_tokens, 470)
  assert.equal(run.result.total_usage.output_tokens, 85)
  const toolResults = run.detail.messages.flatMap(m => m.content).filter(b => b.type === 'tool_result')
  assert.deepEqual(toolResults.map(b => b.tool_use_id), ['d1', 'd2', 'd3', 'd4', 'd5'])
  assert.ok(toolResults.every(b => !b.is_error))
})

test('read-only delegate batching stops at duplicate (kind, task) keys', async t => {
  const f = await fixture(t)
  let parentRound = 0, inFlight = 0, maxInFlight = 0, wave2Arrived = 0
  let firstCallId = null, duplicateSawFirstActive = false
  let releaseWave2
  const wave2Barrier = new Promise(resolve => { releaseWave2 = resolve })
  const sockets = new Set()
  const server = http.createServer((req, res) => {
    let body = ''
    req.on('data', d => { body += d })
    req.on('end', async () => {
      const payload = JSON.parse(body)
      const isChild = JSON.stringify(payload.system).includes('你是子助手。')
      let frames
      if (!isChild) {
        parentRound++
        frames = parentRound === 1
          ? anthropicFrames({
              tools: [
                {id: 'd1', name: 'delegate', arguments: {kind: 'explore', task: 'dup-task'}},
                {id: 'd2', name: 'delegate', arguments: {kind: 'explore', task: 'dup-task'}},
                {id: 'd3', name: 'delegate', arguments: {kind: 'explore', task: 'other-task'}},
              ],
              input: 10,
              output: 5,
            })
          : anthropicFrames({text: 'done', input: 10, output: 5})
      } else {
        const text = JSON.stringify(payload.messages)
        const id = ++inFlight
        if (inFlight > maxInFlight) maxInFlight = inFlight
        if (text.includes('dup-task') && firstCallId === null) {
          firstCallId = id
          await delay(80)
          firstCallId = 'finished'
        } else {
          if (text.includes('dup-task') && firstCallId !== 'finished') duplicateSawFirstActive = true
          wave2Arrived++
          if (wave2Arrived === 2) releaseWave2()
          await Promise.race([wave2Barrier, delay(3000)])
        }
        frames = anthropicFrames({
          text: JSON.stringify({summary: 'ok', findings: [], evidence_refs: [], unresolved: []}),
          input: 10,
          output: 5,
        })
        inFlight--
      }
      res.writeHead(200, {'content-type': 'text/event-stream'})
      for (const frame of frames) res.write(`data: ${JSON.stringify(frame)}\n\n`)
      res.end()
    })
  })
  server.on('connection', socket => { sockets.add(socket); socket.on('close', () => sockets.delete(socket)) })
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
  t.after(() => { for (const s of sockets) s.destroy(); return new Promise(resolve => server.close(resolve)) })

  const model = await configureService(f, `http://127.0.0.1:${server.address().port}`)
  const run = await f.run({model, text: 'check duplicate key barrier'})
  assert.equal(run.result.exit_reason, 'completed')
  assert.equal(duplicateSawFirstActive, false, 'duplicate (kind, task) must not run concurrently with first')
  assert.equal(maxInFlight, 2, 'remaining distinct pair (dup-task, other-task) runs concurrently')
})
