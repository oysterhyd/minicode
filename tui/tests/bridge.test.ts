import assert from 'node:assert/strict'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import test from 'node:test'
import { spawn } from 'node:child_process'
import { BridgeClient } from '../src/bridge.js'
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
