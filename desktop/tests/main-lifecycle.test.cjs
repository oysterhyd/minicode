const test = require('node:test')
const assert = require('node:assert/strict')
const { EventEmitter } = require('node:events')
const fs = require('node:fs')
const path = require('node:path')
const vm = require('node:vm')

// Objects created inside the vm context have foreign prototypes.
const plain = value => JSON.parse(JSON.stringify(value))

// Exercise the real main-process entrypoint without opening windows or spawning Python.
async function launch({ lock = true, focused = true, files = {} } = {}) {
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
  app.requestSingleInstanceLock = () => lock
  app.getVersion = () => '9.9.9'
  app.quits = 0
  const quit = app.quit
  app.quit = () => { app.quits++; quit() }
  const opened = []
  const shown = []
  const shell = {
    openExternal: async url => { opened.push(url) },
    openPath: async target => { opened.push(target); return '' },
    showItemInFolder: target => { opened.push(target) },
  }
  class Notification extends EventEmitter {
    static isSupported() { return true }
    constructor(options) { super(); this.options = options }
    show() { shown.push(this) }
  }
  const screen = { getAllDisplays: () => [{ workArea: { x: 0, y: 0, width: 1920, height: 1080 } }] }
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
        setWindowOpenHandler(handler) { this.openHandler = handler },
        on() {},
        send(channel, event) {
          if (this.destroyed) throw new TypeError('Object has been destroyed')
          messages.push({ channel, event })
        },
      }
    }
    isDestroyed() { return this.destroyed }
    isFocused() { return focused }
    setBackgroundColor(color) { this.background = color }
    setTitleBarOverlay(overlay) { this.overlay = overlay }
    show() { this.visible = true }
    focus() {}
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
      if (name === 'electron') return { app, BrowserWindow, Notification, screen, shell, dialog: {}, ipcMain: { handle: (name, handler) => handlers.set(name, handler) } }
      if (name === 'node:child_process') return { spawn: () => bridge }
      if (name === 'node:readline') return { createInterface: () => lines }
      if (name === 'node:fs') return { ...fs, existsSync: () => false,
        readFileSync: (file, ...rest) => { const key = path.basename(String(file)); if (key in files) return JSON.stringify(files[key]); return fs.readFileSync(file, ...rest) },
        statSync: (file, ...rest) => String(file).startsWith('/fake-dir') ? { isDirectory: () => true } : fs.statSync(file, ...rest),
        realpathSync: (file, ...rest) => String(file).startsWith('/fake-dir') ? String(file) : fs.realpathSync(file, ...rest),
        writeFileSync: () => {} }
      return require(name)
    },
  }, { filename })
  await Promise.resolve()
  return { app, window, bridge, lines, messages, opened, shown, handlers,
    request: (method, params = {}) => handlers.get('desktop:request')({}, method, params) }
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

test('a second instance quits without opening a window', async () => {
  const { app, window, handlers } = await launch({ lock: false })
  assert.equal(window, undefined)
  assert.equal(handlers.size, 0)
  assert.ok(app.quits >= 1)
})

test('creates a hidden, sandboxed window with a 40px title bar', async () => {
  const { window } = await launch()
  assert.equal(window.options.show, false)
  assert.equal(window.options.webPreferences.sandbox, true)
  assert.equal(window.options.webPreferences.spellcheck, false)
  assert.equal(window.options.titleBarOverlay.height, 40)
  window.emit('ready-to-show')
  assert.equal(window.visible, true)
})

test('setAppearance uses the new palette', async () => {
  const { window, request } = await launch()
  await request('setAppearance', { theme: 'light' })
  assert.equal(window.background, '#f5f5f3')
  assert.deepEqual(plain(window.overlay), { color: '#f5f5f3', symbolColor: '#1f1f22', height: 40 })
  await assert.rejects(request('setAppearance', { theme: 'blue' }), /无效/)
})

test('window open handler denies popups and opens http links externally', async () => {
  const { window, opened } = await launch()
  assert.deepEqual(plain(window.contents.openHandler({ url: 'https://example.com' })), { action: 'deny' })
  assert.deepEqual(plain(window.contents.openHandler({ url: 'file:///etc/passwd' })), { action: 'deny' })
  await Promise.resolve()
  assert.deepEqual(opened, ['https://example.com'])
})

test('openExternal only allows http, https and mailto', async () => {
  const { request, opened } = await launch()
  assert.equal(await request('openExternal', { url: 'https://example.com/a' }), true)
  assert.equal(await request('openExternal', { url: 'mailto:dev@example.com' }), true)
  await assert.rejects(request('openExternal', { url: 'file:///C:/Windows' }), /只能打开/)
  await assert.rejects(request('openExternal', { url: 'javascript:alert(1)' }), /只能打开/)
  await assert.rejects(request('openExternal', { url: 42 }), /只能打开/)
  assert.deepEqual(opened, ['https://example.com/a', 'mailto:dev@example.com'])
})

test('recent workspaces migrate the legacy settings file and can be removed', async () => {
  const { request } = await launch({ files: { 'workspace.json': { workspace: '/fake-dir/a' } } })
  assert.equal(await request('getWorkspace'), '/fake-dir/a')
  assert.deepEqual(await request('recentWorkspaces'), ['/fake-dir/a'])
  await request('setWorkspace', { workspace: '/fake-dir/b' })
  assert.deepEqual(await request('recentWorkspaces'), ['/fake-dir/b', '/fake-dir/a'])
  await request('setWorkspace', { workspace: '/fake-dir/a' })
  assert.deepEqual(await request('recentWorkspaces'), ['/fake-dir/a', '/fake-dir/b'])
  assert.deepEqual(await request('removeRecentWorkspace', { workspace: '/fake-dir/b' }), ['/fake-dir/a'])
})

test('notify only shows while unfocused and routes clicks back to the session', async () => {
  const focusedApp = await launch({ focused: true })
  assert.equal(await focusedApp.request('notify', { title: 'Done', body: 'x' }), false)
  const { request, shown, messages } = await launch({ focused: false })
  assert.equal(await request('notify', { title: 'Done', body: 'ok', sessionId: 's1' }), true)
  assert.equal(shown.length, 1)
  assert.equal(shown[0].options.title, 'Done')
  shown[0].emit('click')
  assert.deepEqual(plain(messages.at(-1).event), { event: 'notification_click', sessionId: 's1' })
})

test('getAppInfo reports versions', async () => {
  const { request } = await launch()
  const info = await request('getAppInfo')
  assert.equal(info.version, '9.9.9')
  assert.equal(info.platform, process.platform)
})

test('unknown methods are still forwarded to the bridge', async () => {
  const { request, lines } = await launch()
  const pending = request('listSessions')
  lines.emit('line', JSON.stringify({ id: 1, result: ['ok'] }))
  assert.deepEqual(plain(await pending), ['ok'])
})
