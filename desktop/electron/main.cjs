const { app, BrowserWindow, Notification, dialog, ipcMain, screen, shell } = require('electron')
const { spawn, execFile } = require('node:child_process')
const fs = require('node:fs')
const path = require('node:path')
const readline = require('node:readline')
const { setTimeout, clearTimeout } = require('node:timers')
const utils = require(path.join(__dirname, 'git-utils.cjs'))
const { bridgeLaunch } = require(path.join(__dirname, 'bundled-runtime.cjs'))

const projectRoot = path.resolve(__dirname, '../..')
const devServer = 'http://127.0.0.1:5173'
const isDev = !app.isPackaged && process.argv.includes('--dev')
const APPEARANCE = {
  dark: { background: '#141416', symbol: '#e8e8ea' },
  light: { background: '#f5f5f3', symbol: '#1f1f22' },
}
const TITLEBAR_HEIGHT = 40
const LIST_LIMIT = 5000
const PREVIEW_LIMIT = 1024 * 1024
const DEFAULT_BOUNDS = { width: 1500, height: 940 }

if (process.platform === 'win32') app.setAppUserModelId('com.minicode.desktop')
let window
let bridge
let nextId = 1
let workspace = null
let recent = []
let quitting = false
let settingsPath = null
let toolEnvironment = process.env
const pending = new Map()
// Keep shown notifications referenced so their click handlers survive GC.
const notifications = new Set()

const singleInstance = typeof app.requestSingleInstanceLock === 'function' ? app.requestSingleInstanceLock() : true
if (!singleInstance) app.quit()
else app.on('second-instance', () => focusWindow())

function focusWindow() {
  if (!window || window.isDestroyed()) return
  if (window.isMinimized?.()) window.restore()
  window.show?.()
  window.focus?.()
}

function rejectPending(error) {
  for (const request of pending.values()) request.reject(error)
  pending.clear()
}

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
    execFile(file, args, { cwd, env: toolEnvironment, maxBuffer: 32 * 1024 * 1024, encoding: 'utf8', timeout: 30000, windowsHide: true }, (error, stdout, stderr) => {
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
  const needle = String(query || '').toLowerCase()
  return Array.from(new Set(result.stdout.split('\0'))).filter((name) => name && name.toLowerCase().includes(needle)).slice(0, LIST_LIMIT)
}

async function walk(root, query = '') {
  const matches = []
  const queue = ['']
  const needle = String(query || '').toLowerCase()
  while (queue.length && matches.length < LIST_LIMIT) {
    const prefix = queue.pop()
    let entries
    try { entries = await fs.promises.readdir(path.join(root, prefix), { withFileTypes: true }) }
    catch { continue }
    for (const entry of entries) {
      if (entry.name.startsWith('.') || ['node_modules', 'dist', 'build', '__pycache__'].includes(entry.name)) continue
      const relative = path.join(prefix, entry.name)
      if (entry.isDirectory()) queue.push(relative)
      else if (entry.isFile() && relative.toLowerCase().includes(needle)) matches.push(relative.replaceAll('\\', '/'))
      if (matches.length >= LIST_LIMIT) break
    }
  }
  return matches
}

async function untrackedLines(root, relative) {
  try {
    const target = path.join(root, relative)
    const stat = await fs.promises.stat(target)
    if (!stat.isFile() || stat.size > PREVIEW_LIMIT) return 0
    return utils.countLines(await fs.promises.readFile(target))
  } catch { return 0 }
}

async function numstat(root) {
  const base = ['-c', 'core.quotePath=false', 'diff', '--numstat', '-z', '--no-ext-diff']
  const head = await run('git', [...base, 'HEAD'], root)
  if (head.ok) return utils.parseNumstat(head.stdout)
  // No HEAD yet (fresh repository): staged content plus unstaged work.
  const [staged, unstaged] = await Promise.all([run('git', [...base, '--cached'], root), run('git', base, root)])
  const stats = utils.parseNumstat(staged.ok ? staged.stdout : '')
  for (const [file, value] of utils.parseNumstat(unstaged.ok ? unstaged.stdout : '')) {
    const previous = stats.get(file)
    stats.set(file, previous ? { additions: previous.additions + value.additions, deletions: previous.deletions + value.deletions } : value)
  }
  return stats
}

async function changes(root = workspace) {
  if (!root) return []
  const status = await run('git', ['-c', 'core.quotePath=false', 'status', '--porcelain=v1', '-z', '-uall'], root)
  if (!status.ok) {
    if (/not a git repository/i.test(status.stderr)) return []
    throw new Error(status.stderr || status.error || '无法读取 Git 改动')
  }
  const entries = utils.parseStatus(status.stdout)
  const stats = entries.length ? await numstat(root) : new Map()
  return Promise.all(entries.map(async (entry) => {
    const stat = stats.get(entry.path)
    if (stat) return { ...entry, ...stat }
    if (entry.status === '??') return { ...entry, additions: await untrackedLines(root, entry.path), deletions: 0 }
    return { ...entry, additions: 0, deletions: 0 }
  }))
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
  const handle = await fs.promises.open(target, 'r')
  try {
    const size = (await handle.stat()).size
    const buffer = Buffer.alloc(Math.min(size, PREVIEW_LIMIT))
    const { bytesRead } = await handle.read(buffer, 0, buffer.length, 0)
    const content = buffer.subarray(0, bytesRead).toString('utf8')
    return { text: content, truncated: size > bytesRead }
  } finally {
    await handle.close()
  }
}

async function unstage(relative, root) {
  insideWorkspace(relative, root)
  const restored = await run('git', ['restore', '--staged', '--', relative], root)
  if (restored.ok) return true
  // Older Git has no `restore`.
  const reset = await run('git', ['reset', '-q', 'HEAD', '--', relative], root)
  if (!reset.ok) throw new Error(reset.stderr || restored.stderr || '无法取消暂存')
  return true
}

// -- persisted settings ---------------------------------------------------

function readJson(file) {
  try { return JSON.parse(fs.readFileSync(file, 'utf8')) } catch { return null }
}

function writeJson(file, value) {
  if (!file) return
  try { fs.writeFileSync(file, JSON.stringify(value)) } catch { /* Preferences are best effort. */ }
}

function isDirectory(value) {
  try { return fs.statSync(value).isDirectory() } catch { return false }
}

function saveSettings() { writeJson(settingsPath, { workspace, recent }) }

function useWorkspace(value) {
  workspace = value
  recent = utils.addRecent(recent, value)
  saveSettings()
  return workspace
}

function loadSettings() {
  const settings = utils.normalizeSettings(readJson(settingsPath))
  recent = settings.recent
  if (settings.workspace && isDirectory(settings.workspace)) {
    try { workspace = fs.realpathSync(settings.workspace) } catch { workspace = null }
  }
}

function windowStatePath() { return path.join(app.getPath('userData'), 'window-state.json') }

function initialBounds() {
  const saved = readJson(windowStatePath())
  if (!saved) return { bounds: DEFAULT_BOUNDS, maximized: false }
  let areas = []
  try { areas = screen.getAllDisplays().map(display => display.workArea) } catch { areas = [] }
  const bounds = saved.bounds
  if (!utils.boundsVisible(bounds, areas)) return { bounds: DEFAULT_BOUNDS, maximized: Boolean(saved.maximized) }
  return { bounds: { x: bounds.x, y: bounds.y, width: Math.max(1030, bounds.width), height: Math.max(680, bounds.height) }, maximized: Boolean(saved.maximized) }
}

function trackWindowState(target) {
  let timer = null
  const save = () => {
    if (timer) { clearTimeout(timer); timer = null }
    if (target.isDestroyed()) return
    const maximized = target.isMaximized?.() || false
    // Normal bounds keep the pre-maximize rectangle when maximized.
    const bounds = (maximized && target.getNormalBounds ? target.getNormalBounds() : target.getBounds?.()) || null
    if (bounds) writeJson(windowStatePath(), { bounds, maximized })
  }
  const schedule = () => { if (timer) clearTimeout(timer); timer = setTimeout(save, 400) }
  target.on('resize', schedule)
  target.on('move', schedule)
  target.on('close', save)
}

// -- desktop integration --------------------------------------------------

function notify({ title, body, sessionId } = {}) {
  if (!Notification?.isSupported?.()) return false
  if (window && !window.isDestroyed() && window.isFocused?.()) return false
  const notification = new Notification({ title: String(title || 'MiniCode'), body: String(body || ''), silent: false })
  notifications.add(notification)
  const release = () => notifications.delete(notification)
  notification.on('click', () => {
    release()
    focusWindow()
    sendDesktopEvent({ event: 'notification_click', sessionId: sessionId || undefined })
  })
  notification.on('close', release)
  notification.show()
  return true
}

async function openExternal(url) {
  if (!utils.isAllowedExternal(url)) throw new Error('只能打开 http、https 或 mailto 链接')
  await shell.openExternal(url)
  return true
}

async function openPath(target) {
  const error = await shell.openPath(target)
  if (error) throw new Error(error)
  return true
}

function isAppUrl(url) { return utils.isAppUrl(url, isDev ? devServer : null) }

function secureContents(contents) {
  contents.setWindowOpenHandler?.(({ url }) => {
    if (/^https?:/i.test(url)) void shell.openExternal(url).catch(() => {})
    return { action: 'deny' }
  })
  contents.on?.('will-navigate', (event, url) => {
    if (isAppUrl(url)) return
    event.preventDefault()
    if (/^https?:/i.test(url)) void shell.openExternal(url).catch(() => {})
  })
}

function startBridge() {
  const launch = bridgeLaunch({ packaged: Boolean(app.isPackaged), resourcesPath: process.resourcesPath,
    projectRoot, userData: app.getPath('userData') })
  toolEnvironment = launch.env
  bridge = spawn(launch.file, launch.args, {
    cwd: launch.cwd,
    env: launch.env,
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
      rejectPending(new Error(`Agent bridge 返回了无效消息: ${error}`))
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
    rejectPending(new Error(`Agent bridge exited (${code})`))
    sendDesktopEvent({ event: 'bridge_error', error: `Agent bridge exited (${code})` })
  })
  bridge.on('error', (error) => {
    rejectPending(error)
    sendDesktopEvent({ event: 'bridge_error', error: String(error) })
  })
}

// Methods served by the main process; everything else is forwarded to the bridge.
const local = {
  setAppearance(params) {
    if (!['light', 'dark'].includes(params.theme)) throw new Error('无效的主题')
    const colors = APPEARANCE[params.theme]
    window.setBackgroundColor(colors.background)
    window.setTitleBarOverlay?.({ color: colors.background, symbolColor: colors.symbol, height: TITLEBAR_HEIGHT })
    return true
  },
  getWorkspace: () => workspace,
  closeWindow() { window.close(); return true },
  setWorkspace(params) {
    if (!isDirectory(params.workspace)) throw new Error('工作区不是目录')
    return useWorkspace(fs.realpathSync(params.workspace))
  },
  recentWorkspaces() {
    recent = recent.filter(isDirectory).slice(0, utils.RECENT_LIMIT)
    return recent
  },
  removeRecentWorkspace(params) {
    recent = utils.removeRecent(recent, params.workspace)
    saveSettings()
    return recent
  },
  getAppInfo: () => ({ version: app.getVersion(), electron: process.versions.electron, chrome: process.versions.chrome, platform: process.platform }),
  notify,
  openExternal: params => openExternal(params.url),
  revealPath(params) { shell.showItemInFolder(insideWorkspace(params.path, params.workspace || workspace)); return true },
  openPath: params => openPath(insideWorkspace(params.path, params.workspace || workspace)),
  openWorkspace(params) {
    const root = params.workspace || workspace
    if (!root) throw new Error('请先选择工作区')
    return openPath(root)
  },
  listFiles: params => files(params.query, params.workspace || workspace),
  async readFile(params) {
    const preview = await readPreview(insideWorkspace(params.path, params.workspace || workspace))
    return preview.text + (preview.truncated ? '\n...[文件预览已截断，使用 Agent 的 read 工具分页查看]' : '')
  },
  changes: params => changes(params.workspace || workspace),
  diff: params => diff(params.path, params.workspace || workspace),
  async confirmDiff(params) {
    const root = params.workspace || workspace
    insideWorkspace(params.path, root)
    const result = await run('git', ['add', '--', params.path], root)
    if (!result.ok) throw new Error(result.stderr)
    return true
  },
  async stageAll(params) {
    const root = params.workspace || workspace
    if (!root) throw new Error('请先选择工作区')
    const result = await run('git', ['add', '-A'], root)
    if (!result.ok) throw new Error(result.stderr || result.error)
    return true
  },
  unstageFile: params => unstage(params.path, params.workspace || workspace),
  // Dropped files become @mentions; only paths inside the workspace get a relative form.
  relativePath(params) {
    const root = params.workspace || workspace
    if (!root || typeof params.path !== 'string') return null
    try {
      const relative = path.relative(fs.realpathSync(root), insideWorkspace(params.path, root))
      return relative ? relative.split(path.sep).join('/') : null
    } catch { return null }
  },
}

if (singleInstance) app.whenReady().then(() => {
  settingsPath = path.join(app.getPath('userData'), 'workspace.json')
  loadSettings()
  startBridge()
  ipcMain.handle('desktop:choose-workspace', async () => {
    const result = await dialog.showOpenDialog(window, { properties: ['openDirectory'] })
    // A cancelled dialog must not look like a fresh choice: the frontend treats
    // any non-null result as a new workspace and would start a task view.
    if (result.canceled) return null
    return useWorkspace(result.filePaths[0])
  })
  ipcMain.handle('desktop:choose-acceptance', async () => {
    const result = await dialog.showOpenDialog(window, { properties: ['openFile'],
      filters: [{ name: 'YAML acceptance', extensions: ['yaml', 'yml'] }] })
    return result.canceled ? null : result.filePaths[0]
  })
  ipcMain.handle('desktop:request', async (_event, method, params = {}) => {
    if (quitting || !window || window.isDestroyed()) throw new Error('应用正在关闭')
    const handler = Object.prototype.hasOwnProperty.call(local, method) ? local[method] : null
    return handler ? handler(params || {}) : callBridge(method, params)
  })
  const { bounds, maximized } = initialBounds()
  window = new BrowserWindow({
    ...bounds, minWidth: 1030, minHeight: 680,
    show: false,
    icon: path.join(__dirname, 'app-icon.ico'),
    backgroundColor: APPEARANCE.dark.background,
    titleBarStyle: 'hidden',
    titleBarOverlay: { color: APPEARANCE.dark.background, symbolColor: APPEARANCE.dark.symbol, height: TITLEBAR_HEIGHT },
    autoHideMenuBar: true,
    webPreferences: { preload: path.join(__dirname, 'preload.cjs'), contextIsolation: true, nodeIntegration: false, sandbox: true, spellcheck: false },
  })
  window.once('ready-to-show', () => {
    if (!window || window.isDestroyed()) return
    if (maximized) window.maximize()
    window.show()
  })
  trackWindowState(window)
  secureContents(window.webContents)
  window.on('closed', () => { window = null })
  if (isDev) window.loadURL(devServer)
  else window.loadFile(path.join(__dirname, '../dist/index.html'))
})

app.on('window-all-closed', () => app.quit())
app.on('before-quit', () => {
  if (quitting) return
  quitting = true
  rejectPending(new Error('应用正在关闭'))
  bridge?.kill()
})
