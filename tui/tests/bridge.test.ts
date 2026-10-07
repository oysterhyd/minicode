import assert from 'node:assert/strict'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import test from 'node:test'
import { spawn } from 'node:child_process'
import { BridgeClient } from '../src/bridge.js'
import { bridgeLaunch } from '../src/launch.js'

test('packaged launch uses isolated python and writable cwd', () => {
  const environment = { Path: 'system-tools', PYTHONHOME: 'foreign-python', PYTHONPATH: 'foreign-modules', ANTHROPIC_API_KEY: 'test-key' }
  const launch = bridgeLaunch({ resourcesPath: '/installed/resources', userData: '/profile', environment })
  assert.equal(launch.file, path.join('/installed/resources/runtime/python', process.platform === 'win32' ? 'python.exe' : 'python'))
  assert.equal(launch.cwd, '/profile')
  assert.deepEqual(launch.args.slice(0, 4), ['-I', '-X', 'utf8', '-u'])
  assert.equal(launch.env.PYTHONHOME, undefined)
  assert.equal(launch.env.PYTHONPATH, undefined)
  assert.equal(launch.env.ANTHROPIC_API_KEY, 'test-key')
})

test('source launch uses the Python interpreter selected by the CLI', () => {
  const launch = bridgeLaunch({
    projectRoot: '/checkout',
    environment: { MINICODE_PYTHON: '/selected/python' },
  })
  assert.equal(launch.file, '/selected/python')
  assert.equal(launch.env.PYTHONPATH, path.join('/checkout', 'src'))
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
