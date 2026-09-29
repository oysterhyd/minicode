const test = require('node:test')
const assert = require('node:assert/strict')
const { EventEmitter } = require('node:events')
const fs = require('node:fs')
const path = require('node:path')
const vm = require('node:vm')

// Exercise the real main-process entrypoint without opening windows or spawning Python.
async function launch() {
  const messages = []
  const handlers = new Map()
  const lines = new EventEmitter()
  const bridge = new EventEmitter()
  bridge.exitCode = null
  bridge.stdin = { destroyed: false, write: (_text, callback) => callback() }
  bridge.stdout = new EventEmitter()
  bridge.stderr = new EventEmitter()
  bridge.kills = 0
  bridge.kill = () => { bridge.kills++; bridge.exitCode = 0; bridge.emit('exit', 0) }
  const app = new EventEmitter()
  app.whenReady = () => Promise.resolve()
  app.getPath = () => '/unused-test-profile'
  app.quit = () => app.emit('before-quit')
  app.setAppUserModelId = id => { app.userModelId = id }
  let window
  class BrowserWindow extends EventEmitter {
    constructor(options) {
      super()
      window = this
      this.options = options
      this.destroyed = false
      this.contents = {
        destroyed: false,
        isDestroyed() { return this.destroyed },
        send(channel, event) {
          if (this.destroyed) throw new TypeError('Object has been destroyed')
          messages.push({ channel, event })
        },
      }
    }
    isDestroyed() { return this.destroyed }
    get webContents() {
      if (this.destroyed) throw new TypeError('Object has been destroyed')
      return this.contents
    }
    close() { this.destroyed = true; this.contents.destroyed = true; this.emit('closed') }
    loadFile() {}
    loadURL() {}
  }
  const filename = path.resolve(__dirname, '../electron/main.cjs')
  vm.runInNewContext(fs.readFileSync(filename, 'utf8'), {
    __dirname: path.dirname(filename), process, Buffer,
    require: name => {
      if (name === 'electron') return { app, BrowserWindow, dialog: {}, ipcMain: { handle: (name, handler) => handlers.set(name, handler) } }
      if (name === 'node:child_process') return { spawn: () => bridge }
      if (name === 'node:readline') return { createInterface: () => lines }
      if (name === 'node:fs') return { ...fs, existsSync: () => false }
      return require(name)
    },
  }, { filename })
  await Promise.resolve()
  return { app, window, bridge, lines, messages, request: method => handlers.get('desktop:request')({}, method, {}) }
}

test('uses the MiniCode icon for the Windows taskbar', async () => {
  const { app, window } = await launch()
  if (process.platform === 'win32') assert.equal(app.userModelId, 'com.minicode.desktop')
  assert.match(window.options.icon, /app-icon\.ico$/)
})

test('forwards bridge output and unexpected exit while the window is alive', async () => {
  const { bridge, lines, messages } = await launch()
  lines.emit('line', JSON.stringify({ event: 'text_delta', text: 'hello' }))
  bridge.stderr.emit('data', Buffer.from('stderr'))
  bridge.emit('exit', 1)
  assert.deepEqual(messages.map(message => message.event.event), ['text_delta', 'bridge_error', 'bridge_error'])
  assert.match(messages.at(-1).event.error, /exited \(1\)/)
})

test('late stdout, stderr, errors and exit are safe after window closure', async () => {
  const { window, bridge, lines, messages } = await launch()
  window.close()
  assert.doesNotThrow(() => {
    lines.emit('line', JSON.stringify({ event: 'run_done' }))
    lines.emit('line', 'invalid JSON')
    bridge.stderr.emit('data', Buffer.from('late stderr'))
    bridge.emit('error', new Error('late process error'))
    bridge.emit('exit', 0)
  })
  assert.equal(messages.length, 0)
})

test('does not access destroyed webContents before the window closed event', async () => {
  const { window, bridge, messages } = await launch()
  window.contents.destroyed = true
  assert.doesNotThrow(() => bridge.stderr.emit('data', Buffer.from('late stderr')))
  window.destroyed = true
  assert.doesNotThrow(() => bridge.emit('exit', 0))
  assert.equal(messages.length, 0)
})

test('bridge exit after closing still rejects outstanding IPC requests', async () => {
  const { window, bridge, request } = await launch()
  const rejected = assert.rejects(request('getState'), /exited/)
  window.close()
  bridge.emit('exit', 0)
  await rejected
})

test('normal quit rejects pending requests, suppresses exit errors and stops once', async () => {
  const { app, bridge, request, messages } = await launch()
  const rejected = assert.rejects(request('getState'), /关闭|exited/)
  app.emit('before-quit')
  app.emit('before-quit')
  await rejected
  assert.equal(bridge.kills, 1)
  assert.equal(messages.length, 0)
  await assert.rejects(request('getState'), /关闭|不可用/)
})
