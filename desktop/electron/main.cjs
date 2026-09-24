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
let quitting = false
const pending = new Map()

function sendDesktopEvent(event) {
  // A BrowserWindow can still be referenced after its native window is gone.
  if (quitting || !window || window.isDestroyed()) return
  const contents = window.webContents
  if (!contents.isDestroyed()) contents.send('desktop:event', event)
}

function callBridge(method, params = {}) {
  if (quitting || !bridge || bridge.exitCode !== null || bridge.stdin.destroyed) {
    return Promise.reject(new Error('Agent bridge 不可用'))
  }
  const id = nextId++
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject })
    try {
      bridge.stdin.write(JSON.stringify({ id, method, params }) + '\n', (error) => {
        if (error && pending.has(id)) { pending.delete(id); reject(error) }
      })
    } catch (error) { pending.delete(id); reject(error) }
  })
}

function run(file, args, cwd) {
  return new Promise((resolve) => {
    execFile(file, args, { cwd, maxBuffer: 8 * 1024 * 1024, encoding: 'utf8', timeout: 30000, windowsHide: true }, (error, stdout, stderr) => {
      resolve({ ok: !error, stdout, stderr, error: error ? error.message : '' })
    })
  })
}

function insideWorkspace(relative, rootPath = workspace) {
  if (!rootPath || typeof relative !== 'string') throw new Error('请先选择工作区')
  const unresolved = path.resolve(rootPath, relative)
  const root = fs.realpathSync(rootPath)
  let ancestor = unresolved
  while (!fs.existsSync(ancestor)) {
    const parent = path.dirname(ancestor)
    if (parent === ancestor) throw new Error('无法解析文件路径')
    ancestor = parent
  }
  const target = path.resolve(fs.realpathSync(ancestor), path.relative(ancestor, unresolved))
  const checkedRoot = process.platform === 'win32' ? root.toLowerCase() : root
  const checkedTarget = process.platform === 'win32' ? target.toLowerCase() : target
  if (checkedTarget !== checkedRoot && !checkedTarget.startsWith(checkedRoot + path.sep)) throw new Error('文件不在工作区内')
  return target
}

async function files(query = '', root = workspace) {
  if (!root) return []
  const result = await run('git', ['ls-files', '-z', '--cached', '--others', '--exclude-standard'], root)
  if (!result.ok) return walk(root, query)
  return result.stdout.split('\0').filter((name) => name && name.toLowerCase().includes(query.toLowerCase())).slice(0, 250)
}

async function walk(root, query = '') {
  const matches = []
  const pending = ['']
  const needle = query.toLowerCase()
  while (pending.length && matches.length < 250) {
    const prefix = pending.pop()
    let entries
    try { entries = await fs.promises.readdir(path.join(root, prefix), { withFileTypes: true }) }
    catch { continue }
    for (const entry of entries) {
      if (entry.name.startsWith('.') || ['node_modules', 'dist', 'build', '__pycache__'].includes(entry.name)) continue
      const relative = path.join(prefix, entry.name)
      if (entry.isDirectory()) pending.push(relative)
      else if (entry.isFile() && relative.toLowerCase().includes(needle)) matches.push(relative.replaceAll('\\', '/'))
      if (matches.length >= 250) break
    }
  }
  return matches
}

async function changes(root = workspace) {
  if (!root) return []
  const status = await run('git', ['-c', 'core.quotePath=false', 'status', '--porcelain=v1', '-z', '-uall'], root)
  if (!status.ok) {
    if (/not a git repository/i.test(status.stderr)) return []
    throw new Error(status.stderr || status.error || '无法读取 Git 改动')
  }
  const fields = status.stdout.split('\0')
  const result = []
  for (let index = 0; index < fields.length; index++) {
    const record = fields[index]
    if (!record) continue
    const state = record.slice(0, 2)
    result.push({ status: state, path: record.slice(3) })
    if (state.includes('R') || state.includes('C')) index++ // old path follows the new path
  }
  return result
}

async function diff(relative, root = workspace) {
  const target = insideWorkspace(relative, root)
  const result = await run('git', ['diff', 'HEAD', '--no-ext-diff', '--', relative], root)
  if (!result.ok) throw new Error(result.stderr || result.error || '无法读取差异')
  if (result.stdout.trim()) {
    return result.stdout.length > 200_000
      ? result.stdout.slice(0, 200_000) + '\n...[差异预览已截断，请用 Git 或 Agent 工具查看完整差异]'
      : result.stdout
  }
  if (fs.existsSync(target)) {
    const status = await run('git', ['ls-files', '--error-unmatch', '--', relative], root)
    if (!status.ok) {
      const preview = await readPreview(target)
      if (preview.truncated) return '文件超过 1 MiB，差异预览已省略。可用 Agent 的 read 工具分页查看。'
      const lines = preview.text.replace(/\r\n/g, '\n').split('\n')
      if (lines.at(-1) === '') lines.pop()
      return `--- /dev/null\n+++ b/${relative}\n@@ -0,0 +1,${lines.length} @@\n${lines.map(line => '+' + line).join('\n')}`
    }
  }
  return '暂无未提交差异'
}

async function readPreview(target) {
  const limit = 1024 * 1024
  const handle = await fs.promises.open(target, 'r')
  try {
    const size = (await handle.stat()).size
    const buffer = Buffer.alloc(Math.min(size, limit))
    const { bytesRead } = await handle.read(buffer, 0, buffer.length, 0)
    const content = buffer.subarray(0, bytesRead).toString('utf8')
    return { text: content, truncated: size > bytesRead }
  } finally {
    await handle.close()
  }
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
    let message
    try {
      message = JSON.parse(line)
      if (!message || typeof message !== 'object' || (!('event' in message) && !('id' in message))) {
        throw new Error('缺少事件或请求 ID')
      }
    }
    catch (error) {
      for (const request of pending.values()) request.reject(new Error(`Agent bridge 返回了无效消息: ${error}`))
      pending.clear()
      sendDesktopEvent({ event: 'bridge_error', error: 'Agent bridge 返回了无效消息' })
      return
    }
    if (message.event) sendDesktopEvent(message)
    else {
      const request = pending.get(message.id)
      if (!request) return
      pending.delete(message.id)
      if (message.error) request.reject(new Error(message.error))
      else request.resolve(message.result)
    }
  })
  bridge.stderr.on('data', (chunk) => sendDesktopEvent({ event: 'bridge_error', error: chunk.toString() }))
  bridge.on('exit', (code) => {
    for (const request of pending.values()) request.reject(new Error(`Agent bridge exited (${code})`))
    pending.clear()
    sendDesktopEvent({ event: 'bridge_error', error: `Agent bridge exited (${code})` })
  })
  bridge.on('error', (error) => {
    for (const request of pending.values()) request.reject(error)
    pending.clear()
    sendDesktopEvent({ event: 'bridge_error', error: String(error) })
  })
}

app.whenReady().then(() => {
  const settingsPath = path.join(app.getPath('userData'), 'workspace.json')
  if (fs.existsSync(settingsPath)) {
    try {
      const saved = JSON.parse(fs.readFileSync(settingsPath, 'utf8')).workspace
      if (typeof saved === 'string' && fs.statSync(saved).isDirectory()) workspace = fs.realpathSync(saved)
    } catch { workspace = null }
  }
  startBridge()
  ipcMain.handle('desktop:choose-workspace', async () => {
    const result = await dialog.showOpenDialog(window, { properties: ['openDirectory'] })
    // A cancelled dialog must not look like a fresh choice: the frontend treats
    // any non-null result as a new workspace and would start a task view.
    if (result.canceled) return null
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
    if (quitting || !window || window.isDestroyed()) throw new Error('应用正在关闭')
    if (method === 'setAppearance') {
      if (!['light', 'dark'].includes(params.theme)) throw new Error('无效的主题')
      const dark = params.theme === 'dark'
      window.setBackgroundColor(dark ? '#191a1c' : '#f0eeea')
      window.setTitleBarOverlay({ color: dark ? '#191a1c' : '#f0eeea', symbolColor: dark ? '#e9edeb' : '#262722', height: 32 })
      return true
    }
    if (method === 'getWorkspace') return workspace
    if (method === 'closeWindow') { window.close(); return true }
    if (method === 'setWorkspace') {
      if (!fs.statSync(params.workspace).isDirectory()) throw new Error('工作区不是目录')
      workspace = fs.realpathSync(params.workspace)
      fs.writeFileSync(settingsPath, JSON.stringify({ workspace }))
      return workspace
    }
    if (method === 'listFiles') return files(params.query, params.workspace || workspace)
    if (method === 'readFile') {
      const preview = await readPreview(insideWorkspace(params.path, params.workspace || workspace))
      return preview.text + (preview.truncated ? '\n...[文件预览已截断，使用 Agent 的 read 工具分页查看]' : '')
    }
    if (method === 'changes') return changes(params.workspace || workspace)
    if (method === 'diff') return diff(params.path, params.workspace || workspace)
    if (method === 'confirmDiff') {
      insideWorkspace(params.path, params.workspace || workspace)
      const result = await run('git', ['add', '--', params.path], params.workspace || workspace)
      if (!result.ok) throw new Error(result.stderr)
      return true
    }
    return callBridge(method, params)
  })
  window = new BrowserWindow({
    width: 1500, height: 940, minWidth: 1030, minHeight: 680,
    backgroundColor: '#111111',
    titleBarStyle: 'hidden',
    titleBarOverlay: { color: '#191a1c', symbolColor: '#e9edeb', height: 32 },
    autoHideMenuBar: true,
    webPreferences: { preload: path.join(__dirname, 'preload.cjs'), contextIsolation: true, nodeIntegration: false },
  })
  window.on('closed', () => { window = null })
  if (process.argv.includes('--dev')) window.loadURL('http://127.0.0.1:5173')
  else window.loadFile(path.join(__dirname, '../dist/index.html'))
})

app.on('window-all-closed', () => app.quit())
app.on('before-quit', () => {
  if (quitting) return
  quitting = true
  for (const request of pending.values()) request.reject(new Error('应用正在关闭'))
  pending.clear()
  bridge?.kill()
})
