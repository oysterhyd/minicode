import assert from 'node:assert/strict'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import test from 'node:test'
import { spawn } from 'node:child_process'
import { BridgeClient, protocolProblem } from '../src/bridge.js'
import { bridgeLaunch } from '../src/launch.js'

test('packaged launch uses the native agent and writable cwd', () => {
  const environment = { Path: 'system-tools', ANTHROPIC_API_KEY: 'test-key' }
  const launch = bridgeLaunch({ resourcesPath: '/installed/resources', userData: '/profile', environment })
  assert.equal(launch.file, path.join('/installed/resources/runtime', process.platform === 'win32' ? 'minicode.exe' : 'minicode'))
  assert.equal(launch.cwd, '/profile')
  assert.deepEqual(launch.args, ['bridge'])
  assert.deepEqual(launch.env.Path?.split(path.delimiter), [path.join('/installed/resources/runtime', 'git', 'cmd'), path.join('/installed/resources/runtime', 'node'), 'system-tools'])
  assert.equal(launch.env.ANTHROPIC_API_KEY, 'test-key')
})

test('source launch uses the native backend selected by the CLI', () => {
  const launch = bridgeLaunch({
    projectRoot: '/checkout',
    environment: { MINICODE_AGENT: '/selected/minicode' },
  })
  assert.equal(launch.file, '/selected/minicode')
  assert.deepEqual(launch.args, ['bridge'])
})

test('NDJSON client maps ids and events', async () => {
  const script = `
    const rl = require('node:readline').createInterface({ input: process.stdin })
    rl.on('line', line => {
      const msg = JSON.parse(line)
      if (msg.method === 'initialize') {
        process.stdout.write(JSON.stringify({ event: 'text_delta', text: 'hi' }) + '\\n')
        process.stdout.write(JSON.stringify({ id: msg.id, result: { ok: true } }) + '\\n')
      }
    })
  `
  const file = path.join(os.tmpdir(), `minicode-bridge-${process.pid}.cjs`)
  fs.writeFileSync(file, script)
  const child = spawn(process.execPath, [file], { stdio: ['pipe', 'pipe', 'pipe'] })
  const client = new BridgeClient(child)
  const events: unknown[] = []
  client.onEvent(event => events.push(event))
  const result = await client.request<{ ok: boolean }>('initialize')
  assert.equal(result.ok, true)
  assert.equal((events[0] as { text: string }).text, 'hi')
  client.close()
  await new Promise(resolve => child.on('exit', resolve))
  fs.unlinkSync(file)
})

// B-COMPAT: an agent newer than the TUI may send event types the TUI does not
// know yet; they are ignored and the connection stays usable.
test('unknown bridge events are ignored without failing the connection', async () => {
  const script = `
    const rl = require('node:readline').createInterface({ input: process.stdin })
    rl.on('line', line => {
      const msg = JSON.parse(line)
      process.stdout.write(JSON.stringify({ event: 'future_event', data: 1 }) + '\\n')
      process.stdout.write(JSON.stringify({ id: msg.id, result: { method: msg.method } }) + '\\n')
    })
  `
  const file = path.join(os.tmpdir(), `minicode-bridge-unknown-${process.pid}.cjs`)
  fs.writeFileSync(file, script)
  const child = spawn(process.execPath, [file], { stdio: ['pipe', 'pipe', 'pipe'] })
  const client = new BridgeClient(child)
  const events: { event: string; fatal?: boolean }[] = []
  client.onEvent(event => events.push(event as { event: string; fatal?: boolean }))
  try {
    assert.deepEqual(await client.request('initialize'), { method: 'initialize' })
    assert.deepEqual(await client.request('getState'), { method: 'getState' })
    assert.equal(events.some(event => event.event === 'bridge_error' && event.fatal), false)
    assert.equal(events.some(event => event.event === 'future_event'), false)
    assert.deepEqual([...client.ignoredEvents], ['future_event'])
  } finally {
    await client.close()
    fs.unlinkSync(file)
  }
})

test('protocol versions are compared with an upgrade hint instead of a crash', () => {
  assert.equal(protocolProblem({ protocolVersion: 2 }), null)
  assert.equal(protocolProblem({ state: { protocolVersion: 2 } }), null)
  assert.match(protocolProblem({ protocolVersion: 1 })!.message, /升级 MiniCode agent/)
  assert.equal(protocolProblem({ protocolVersion: 1 })!.level, 'error')
  assert.match(protocolProblem({ protocolVersion: 3 })!.message, /升级 TUI/)
  assert.equal(protocolProblem({ protocolVersion: 3 })!.level, 'warning')
  assert.match(protocolProblem({ protocolVersion: 2, minClientProtocol: 3 })!.message, /升级 TUI/)
})

// B-STDERR: diagnostic text, even text containing "Error", is not a protocol error.
test('stderr and protocol logs go to disk while warnings stay nonfatal', async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'minicode-stderr-'))
  const log = path.join(root, 'agent.log')
  const script = `require('node:readline').createInterface({input:process.stdin}).on('line', line=>{
    const {id}=JSON.parse(line)
    process.stderr.write('ordinary startup log: Error count=0 中文\\n')
    for (const message of [{event:'log',text:'protocol diagnostic'},{event:'warning',text:'optional extension unavailable'},{id,result:true}]) process.stdout.write(JSON.stringify(message)+'\\n')
  })`
  const child=spawn(process.execPath,['-e',script],{stdio:['pipe','pipe','pipe']})
  const client=new BridgeClient(child,35000,log)
  const events: Array<{event:string;text?:string}> = []
  client.onEvent(event=>events.push(event))
  try {
    assert.equal(await client.request('initialize'),true)
    await new Promise(resolve=>setTimeout(resolve,50))
    assert.equal(events.some(e=>e.event==='bridge_error'),false)
    assert.equal(events.some(e=>e.event==='log'),false)
    assert.ok(events.some(e=>e.event==='warning' && e.text==='optional extension unavailable'))
  } finally { await client.close() }
  try {
    assert.match(fs.readFileSync(log,'utf8'),/ordinary startup log: Error count=0 中文/)
    assert.match(fs.readFileSync(log,'utf8'),/protocol diagnostic/)
  } finally { fs.rmSync(root,{recursive:true,force:true}) }
})
