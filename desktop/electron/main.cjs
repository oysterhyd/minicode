const { app, BrowserWindow, dialog, ipcMain } = require('electron')
const { spawn, execFile } = require('node:child_process')
const fs = require('node:fs')
const path = require('node:path')
const readline = require('node:readline')

const projectRoot = path.resolve(__dirname, '../..')
let window
let bridge
let nextId = 1
let workspace = null
const pending = new Map()

function callBridge(method, params = {}) {
  const id = nextId++
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject })
    bridge.stdin.write(JSON.stringify({ id, method, params }) + '\n')
  })
}

function run(file, args, cwd) {
  return new Promise((resolve) => {
    execFile(file, args, { cwd, maxBuffer: 8 * 1024 * 1024, encoding: 'utf8' }, (error, stdout, stderr) => {
      resolve({ ok: !error, stdout, stderr })
    })
  })
}

function insideWorkspace(relative) {
  const unresolved = path.resolve(workspace, relative)
  const root = fs.realpathSync(workspace)
  const target = fs.existsSync(unresolved) ? fs.realpathSync(unresolved) : unresolved
  if (target !== root && !target.startsWith(root + path.sep)) throw new Error('文件不在工作区内')
  return target
}

async function files(query = '') {
  if (!workspace) return []
  const result = await run('git', ['ls-files', '--cached', '--others', '--exclude-standard'], workspace)
  const entries = result.ok ? result.stdout.split(/\r?\n/) : walk(workspace)
  return entries.filter((name) => name && name.toLowerCase().includes(query.toLowerCase())).slice(0, 250)
}

function walk(root, prefix = '') {
  return fs.readdirSync(path.join(root, prefix), { withFileTypes: true }).flatMap((entry) => {
    if (entry.name.startsWith('.') || ['node_modules', 'dist', 'build', '__pycache__'].includes(entry.name)) return []
    const relative = path.join(prefix, entry.name)
    return entry.isDirectory() ? walk(root, relative) : [relative.replaceAll('\\', '/')]
  })
}

async function changes() {
  if (!workspace) return []
  const status = await run('git', ['-c', 'core.quotePath=false', 'status', '--porcelain=v1', '-uall'], workspace)
  if (!status.ok) return []
  return status.stdout.split(/\r?\n/).filter(Boolean).map((line) => ({ status: line.slice(0, 2), path: line.slice(3).split(' -> ').pop() }))
}

async function diff(relative) {
  const target = insideWorkspace(relative)
  const result = await run('git', ['diff', 'HEAD', '--no-ext-diff', '--', relative], workspace)
  if (result.stdout.trim()) return result.stdout
  if (fs.existsSync(target)) {
    const status = await run('git', ['ls-files', '--error-unmatch', '--', relative], workspace)
    if (!status.ok) {
      const lines = fs.readFileSync(target, 'utf8').replace(/\r\n/g, '\n').split('\n')
      if (lines.at(-1) === '') lines.pop()
      return `--- /dev/null\n+++ b/${relative}\n@@ -0,0 +1,${lines.length} @@\n${lines.map(line => '+' + line).join('\n')}`
    }
  }
  return '暂无未提交差异'
}

function startBridge() {
  const venv = process.platform === 'win32' ? path.join(projectRoot, '.venv', 'Scripts', 'python.exe') : path.join(projectRoot, '.venv', 'bin', 'python')
  const python = fs.existsSync(venv) ? venv : process.platform === 'win32' ? 'python' : 'python3'
  bridge = spawn(python, ['-u', path.join(projectRoot, 'desktop', 'bridge.py')], {
    cwd: projectRoot,
    env: { ...process.env, PYTHONPATH: path.join(projectRoot, 'src'), PYTHONIOENCODING: 'utf-8' },
    stdio: ['pipe', 'pipe', 'pipe'],
    windowsHide: true,
  })
  readline.createInterface({ input: bridge.stdout }).on('line', (line) => {
    const message = JSON.parse(line)
    if (message.event) window?.webContents.send('desktop:event', message)
    else {
      const request = pending.get(message.id)
      if (!request) return
      pending.delete(message.id)
      if (message.error) request.reject(new Error(message.error))
      else request.resolve(message.result)
    }
  })
  bridge.stderr.on('data', (chunk) => window?.webContents.send('desktop:event', { event: 'bridge_error', error: chunk.toString() }))
  bridge.on('exit', (code) => window?.webContents.send('desktop:event', { event: 'bridge_error', error: `Agent bridge exited (${code})` }))
}

app.whenReady().then(() => {
  const settingsPath = path.join(app.getPath('userData'), 'workspace.json')
  if (fs.existsSync(settingsPath)) workspace = JSON.parse(fs.readFileSync(settingsPath, 'utf8')).workspace
  startBridge()
  ipcMain.handle('desktop:choose-workspace', async () => {
    const result = await dialog.showOpenDialog(window, { properties: ['openDirectory'] })
    if (result.canceled) return workspace
    workspace = result.filePaths[0]
    fs.writeFileSync(settingsPath, JSON.stringify({ workspace }))
    return workspace
  })
  ipcMain.handle('desktop:choose-acceptance', async () => {
    const result = await dialog.showOpenDialog(window, { properties: ['openFile'],
      filters: [{ name: 'YAML acceptance', extensions: ['yaml', 'yml'] }] })
    return result.canceled ? null : result.filePaths[0]
  })
  ipcMain.handle('desktop:request', async (_event, method, params) => {
    if (method === 'getWorkspace') return workspace
    if (method === 'closeWindow') { window.close(); return true }
    if (method === 'setWorkspace') {
      if (!fs.statSync(params.workspace).isDirectory()) throw new Error('工作区不是目录')
      workspace = fs.realpathSync(params.workspace)
      fs.writeFileSync(settingsPath, JSON.stringify({ workspace }))
      return workspace
    }
    if (method === 'listFiles') return files(params.query)
    if (method === 'readFile') return fs.readFileSync(insideWorkspace(params.path), 'utf8')
    if (method === 'changes') return changes()
    if (method === 'diff') return diff(params.path)
    if (method === 'confirmDiff') {
      insideWorkspace(params.path)
      const result = await run('git', ['add', '--', params.path], workspace)
      if (!result.ok) throw new Error(result.stderr)
      return true
    }
    return callBridge(method, params)
  })
  window = new BrowserWindow({
    width: 1500, height: 940, minWidth: 1030, minHeight: 680,
    backgroundColor: '#111111',
    titleBarStyle: 'hidden',
    titleBarOverlay: { color: '#141414', symbolColor: '#eeeeee', height: 28 },
    autoHideMenuBar: true,
    webPreferences: { preload: path.join(__dirname, 'preload.cjs'), contextIsolation: true, nodeIntegration: false },
  })
  if (process.argv.includes('--dev')) window.loadURL('http://127.0.0.1:5173')
  else window.loadFile(path.join(__dirname, '../dist/index.html'))
})

app.on('window-all-closed', () => app.quit())
app.on('before-quit', () => bridge?.kill())
