const test = require('node:test')
const assert = require('node:assert/strict')
const { EventEmitter } = require('node:events')
const { execFileSync } = require('node:child_process')
const childProcess = require('node:child_process')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')
const vm = require('node:vm')

const plain = value => JSON.parse(JSON.stringify(value))

// Load the real main process with real git and fs; only Electron and the Python bridge are faked.
async function launch() {
  const handlers = new Map()
  const bridge = new EventEmitter()
  Object.assign(bridge, { exitCode: null, stdin: { destroyed: false, write: (_t, cb) => cb() }, stdout: new EventEmitter(), stderr: new EventEmitter(), kill() {} })
  const app = new EventEmitter()
  Object.assign(app, { whenReady: () => Promise.resolve(), getPath: () => os.tmpdir(), quit() {}, setAppUserModelId() {}, requestSingleInstanceLock: () => true, getVersion: () => '0.0.0' })
  class BrowserWindow extends EventEmitter {
    constructor() { super(); this.webContents = { isDestroyed: () => false, send() {}, setWindowOpenHandler() {}, on() {} } }
    isDestroyed() { return false }
    loadFile() {}
    loadURL() {}
  }
  const filename = path.resolve(__dirname, '../electron/main.cjs')
  vm.runInNewContext(fs.readFileSync(filename, 'utf8'), {
    __dirname: path.dirname(filename), process, Buffer,
    require: name => {
      if (name === 'electron') return { app, BrowserWindow, Notification: { isSupported: () => false }, screen: { getAllDisplays: () => [] }, shell: {}, dialog: {}, ipcMain: { handle: (n, h) => handlers.set(n, h) } }
      if (name === 'node:child_process') return { ...childProcess, spawn: () => bridge }
      if (name === 'node:readline') return { createInterface: () => new EventEmitter() }
      if (name === 'node:fs') return { ...fs, writeFileSync() {} }
      return require(name)
    },
  }, { filename })
  await Promise.resolve()
  return (method, params) => handlers.get('desktop:request')({}, method, params)
}

function git(cwd, ...args) {
  execFileSync('git', ['-c', 'user.name=t', '-c', 'user.email=t@t', '-c', 'commit.gpgsign=false', ...args], { cwd, stdio: 'pipe' })
}

function repo() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'minicode-git-'))
  git(root, 'init', '-q')
  return root
}

const byPath = list => Object.fromEntries(plain(list).map(item => [item.path, item]))

test('changes reports numstat for modified, staged, untracked and binary files', async (t) => {
  const root = repo()
  t.after(() => fs.rmSync(root, { recursive: true, force: true }))
  fs.writeFileSync(path.join(root, 'a.txt'), 'one\ntwo\nthree\n')
  fs.writeFileSync(path.join(root, 'gone.txt'), 'x\ny\n')
  git(root, 'add', '-A'); git(root, 'commit', '-q', '-m', 'init')
  fs.writeFileSync(path.join(root, 'a.txt'), 'one\n2\nthree\nfour\n')
  fs.rmSync(path.join(root, 'gone.txt'))
  fs.writeFileSync(path.join(root, 'staged.txt'), 'a\nb\nc')
  git(root, 'add', 'staged.txt')
  fs.writeFileSync(path.join(root, 'new.txt'), 'n1\nn2\n')
  fs.writeFileSync(path.join(root, 'blob.bin'), Buffer.from([1, 0, 2, 10]))

  const request = await launch()
  const changes = byPath(await request('changes', { workspace: root }))
  assert.deepEqual(changes['a.txt'], { status: ' M', path: 'a.txt', additions: 2, deletions: 1 })
  assert.deepEqual(changes['gone.txt'], { status: ' D', path: 'gone.txt', additions: 0, deletions: 2 })
  assert.deepEqual(changes['staged.txt'], { status: 'A ', path: 'staged.txt', additions: 3, deletions: 0 })
  assert.deepEqual(changes['new.txt'], { status: '??', path: 'new.txt', additions: 2, deletions: 0 })
  assert.deepEqual(changes['blob.bin'], { status: '??', path: 'blob.bin', additions: 0, deletions: 0 })
})

test('changes falls back when the repository has no HEAD yet', async (t) => {
  const root = repo()
  t.after(() => fs.rmSync(root, { recursive: true, force: true }))
  fs.writeFileSync(path.join(root, 'first.txt'), '1\n2\n')
  git(root, 'add', 'first.txt')
  fs.writeFileSync(path.join(root, 'first.txt'), '1\n2\n3\n')
  const request = await launch()
  const changes = byPath(await request('changes', { workspace: root }))
  assert.deepEqual(changes['first.txt'], { status: 'AM', path: 'first.txt', additions: 3, deletions: 0 })
})

test('stageAll and unstageFile update the index', async (t) => {
  const root = repo()
  t.after(() => fs.rmSync(root, { recursive: true, force: true }))
  fs.writeFileSync(path.join(root, 'a.txt'), 'a\n')
  git(root, 'add', '-A'); git(root, 'commit', '-q', '-m', 'init')
  fs.writeFileSync(path.join(root, 'a.txt'), 'b\n')
  fs.writeFileSync(path.join(root, 'b.txt'), 'b\n')
  const request = await launch()
  await request('stageAll', { workspace: root })
  let changes = byPath(await request('changes', { workspace: root }))
  assert.equal(changes['a.txt'].status, 'M ')
  assert.equal(changes['b.txt'].status, 'A ')
  await request('unstageFile', { workspace: root, path: 'a.txt' })
  changes = byPath(await request('changes', { workspace: root }))
  assert.equal(changes['a.txt'].status, ' M')
  await assert.rejects(request('unstageFile', { workspace: root, path: '../outside.txt' }), /不在工作区/)
})

test('listFiles is no longer capped at 250 entries', async (t) => {
  const root = repo()
  t.after(() => fs.rmSync(root, { recursive: true, force: true }))
  for (let i = 0; i < 300; i++) fs.writeFileSync(path.join(root, `f${i}.txt`), '')
  const request = await launch()
  assert.equal((await request('listFiles', { workspace: root })).length, 300)
})
