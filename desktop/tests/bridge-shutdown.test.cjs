'use strict'
const { test } = require('node:test')
const assert = require('node:assert/strict')
const { EventEmitter, once } = require('node:events')
const { spawn } = require('node:child_process')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')
const { stopBridge, killBridgeTree } = require('../electron/bridge-shutdown.cjs')

function fake() {
  const child = new EventEmitter()
  child.exitCode = null
  child.ends = 0
  child.stdin = { end() { child.ends++ } }
  return child
}

test('shutdown closes stdin and awaits graceful exit without force killing', async () => {
  const child = fake()
  let killed = false
  const stopped = stopBridge(child, { timeoutMs: 1000, terminateTree: async () => { killed = true } })
  assert.equal(child.ends, 1)
  child.exitCode = 0
  child.emit('exit', 0)
  await stopped
  assert.equal(killed, false)
  assert.equal(child.listenerCount('exit'), 0)
})

test('shutdown deadline falls back to tree termination once', async () => {
  const child = fake()
  let killed = 0
  await stopBridge(child, { timeoutMs: 10, terminateTree: async target => { assert.equal(target, child); killed++ } })
  assert.equal(killed, 1)
  assert.equal(child.listenerCount('exit'), 0)
})

const python = path.resolve(__dirname, '../../.venv/Scripts/python.exe')
test('Windows deadline kills the real Python descendant tree', { skip: process.platform !== 'win32' || !fs.existsSync(python), timeout: 15000 }, async (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'minicode-shutdown-'))
  const marker = path.join(root, 'orphan.txt')
  const descendant = `import time,pathlib; print('child-ready',flush=True); time.sleep(1); pathlib.Path(${JSON.stringify(marker)}).write_text('orphan')`
  const parentCode = `import subprocess,sys,time; p=subprocess.Popen([sys.executable,'-c',${JSON.stringify(descendant)}],stdout=subprocess.PIPE,text=True); print(str(p.pid)+':'+p.stdout.readline().strip(),flush=True); time.sleep(30)`
  const child = spawn(python, ['-u', '-c', parentCode], { stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true })
  const exited = once(child, 'exit')
  t.after(async () => {
    if (child.exitCode === null && !child.signalCode) await killBridgeTree(child)
    fs.rmSync(root, { recursive: true, force: true })
  })
  const [ready] = await once(child.stdout, 'data')
  assert.match(ready.toString(), /\d+:child-ready/)
  await stopBridge(child, { timeoutMs: 20 })
  await exited
  await new Promise(resolve => setTimeout(resolve, 1200))
  assert.equal(fs.existsSync(marker), false, 'descendant must not mutate files after app quit')
})
