// Exercise the installed CLI and real Electron -> preload -> Python IPC with
// an empty home, no provider credentials, and no system Python/Git on PATH.
const assert = require('node:assert/strict')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')
const net = require('node:net')
const { spawn, execFileSync } = require('node:child_process')

const installation = path.resolve(process.argv[2] || path.join(__dirname, 'dist/win-unpacked'))
const scratch = fs.mkdtempSync(path.join(os.tmpdir(), 'minicode-release-'))
const runtime = path.join(installation, 'resources/runtime')
const python = path.join(runtime, 'python/python.exe')
const env = { ...process.env, USERPROFILE: scratch, HOME: scratch, APPDATA: path.join(scratch, 'AppData/Roaming'),
  LOCALAPPDATA: path.join(scratch, 'AppData/Local'), PYTHONUTF8: '1', PYTHONIOENCODING: 'utf-8' }
for (const key of Object.keys(env)) {
  if (/API_?KEY|TOKEN|SECRET|PASSWORD|^PYTHONPATH$|^PYTHONHOME$|^ELECTRON_RUN_AS_NODE$/i.test(key)) delete env[key]
  if (key.toLowerCase() === 'path') delete env[key]
}
env.PATH = [path.join(runtime, 'python'), path.join(runtime, 'python/Scripts'), path.join(runtime, 'git/cmd'),
  path.join(process.env.SystemRoot, 'System32'), path.join(process.env.SystemRoot, 'System32/WindowsPowerShell/v1.0')].join(';')
fs.mkdirSync(env.APPDATA, { recursive: true })
fs.mkdirSync(env.LOCALAPPDATA, { recursive: true })
const run = (file, args) => execFileSync(file, args, { cwd: scratch, env, encoding: 'utf8', windowsHide: true,
  windowsVerbatimArguments: /cmd\.exe$/i.test(file), timeout: 180000 })
const pause = ms => new Promise(resolve => setTimeout(resolve, ms))

async function main() {
  assert.equal(run(python, ['-I', '-c', 'import minicode; print(minicode.__version__)']).trim(), '1.0.0')
  assert.match(run(python, ['-I', '-X', 'utf8', '-m', 'minicode', '--help']), /run/)
  assert.match(run(path.join(runtime, 'git/cmd/git.exe'), ['--version']), /2\.56\.0/)
  assert.match(run(process.env.ComSpec || 'cmd.exe', ['/d', '/s', '/c', `""${path.join(installation, 'minicode.cmd')}" --help"`]), /run/)
  run(python, ['-I', '-X', 'utf8', '-m', 'minicode', 'eval', '--task', 'pagination_bounds', '--baselines', 'b0,b2', '--output', 'results'])
  const results = JSON.parse(fs.readFileSync(path.join(scratch, 'results/results.json'), 'utf8'))
  assert.equal(results.run_count, 2)
  assert.equal(results.passed, 2)
  console.log('Installed CLI, launcher, bundled Git, and offline B0/B2 evaluation passed.')
  const workspace = path.join(scratch, 'workspace')
  fs.mkdirSync(workspace)
  run(path.join(runtime, 'git/cmd/git.exe'), ['-C', workspace, 'init', '-q'])
  fs.writeFileSync(path.join(workspace, 'hello.txt'), 'hello\nworld\n')

  const reservation = net.createServer()
  await new Promise(resolve => reservation.listen(0, '127.0.0.1', resolve))
  const port = reservation.address().port
  await new Promise(resolve => reservation.close(resolve))
  const app = spawn(path.join(installation, 'MiniCode.exe'), [`--remote-debugging-port=${port}`, '--remote-debugging-address=127.0.0.1'],
    { cwd: scratch, env: { ...env, PATH: [path.join(process.env.SystemRoot, 'System32'),
      path.join(process.env.SystemRoot, 'System32/WindowsPowerShell/v1.0')].join(';') },
      windowsHide: true, stdio: ['ignore', 'ignore', 'pipe'] })
  let errors = ''
  app.stderr.on('data', chunk => { errors += chunk.toString() })
  let socket
  try {
    let page
    const deadline = Date.now() + 45000
    while (!page && Date.now() < deadline) {
      if (app.exitCode !== null) throw new Error(`Electron exited: ${app.exitCode}`)
      try { page = (await (await fetch(`http://127.0.0.1:${port}/json`)).json()).find(item => item.type === 'page' && item.url.startsWith('file:')) }
      catch { /* The renderer may still be starting. */ }
      if (!page) await pause(250)
    }
    assert.ok(page, 'packaged renderer did not start')
    socket = new WebSocket(page.webSocketDebuggerUrl)
    await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject })
    let nextId = 0
    const pending = new Map()
    socket.onmessage = message => {
      const data = JSON.parse(message.data)
      const request = pending.get(data.id)
      if (request) { pending.delete(data.id); clearTimeout(request.timer); data.error ? request.reject(new Error(data.error.message)) : request.resolve(data.result) }
    }
    const send = (method, params) => new Promise((resolve, reject) => {
      const id = ++nextId
      const timer = setTimeout(() => { pending.delete(id); reject(new Error(`CDP timeout: ${method}`)) }, 30000)
      pending.set(id, { resolve, reject, timer })
      socket.send(JSON.stringify({ id, method, params }))
    })
    const evaluated = await send('Runtime.evaluate', { awaitPromise: true, returnByValue: true,
      expression: `(async () => {
        while (!window.desktop || !document.querySelector('#root')?.textContent) await new Promise(r => setTimeout(r, 100));
        const info = await window.desktop.request('getAppInfo');
        const initialized = await window.desktop.request('initialize');
        const config = await window.desktop.request('getConfiguration');
        await window.desktop.request('setWorkspace', { workspace: ${JSON.stringify(workspace)} });
        const changes = await window.desktop.request('changes');
        await window.desktop.request('stageAll');
        const staged = await window.desktop.request('changes');
        return { version: info.version, sessions: initialized.sessions.length,
          hasKeys: config.services.some(s => s.hasApiKey || s.apiKey), changes, staged, text: document.body.innerText };
      })()` })
    assert.equal(evaluated.exceptionDetails, undefined, 'IPC call failed')
    const state = evaluated.result.value
    assert.equal(state.version, '1.0.0')
    assert.equal(state.sessions, 0)
    assert.equal(state.hasKeys, false)
    assert.equal(state.changes[0].path, 'hello.txt')
    assert.equal(state.changes[0].additions, 2)
    assert.equal(state.staged[0].status, 'A ')
    assert.ok(state.text.length > 20, 'renderer is blank')
    const screenshot = await send('Page.captureScreenshot', { format: 'png' })
    fs.mkdirSync(path.join(__dirname, '.cache'), { recursive: true })
    fs.writeFileSync(path.join(__dirname, '.cache/installed-smoke.png'), Buffer.from(screenshot.data, 'base64'))
    await send('Runtime.evaluate', { expression: `window.desktop.request('closeWindow')` })
    await Promise.race([new Promise(resolve => app.once('exit', resolve)), pause(10000)])
    console.log('Packaged Desktop renderer and Python IPC passed with an empty user profile.')
  } finally {
    socket?.close()
    if (app.exitCode === null) execFileSync('taskkill', ['/PID', String(app.pid), '/T', '/F'], { stdio: 'ignore', windowsHide: true })
    fs.writeFileSync(path.join(__dirname, '.cache/smoke-stderr.log'), errors)
  }
  console.log(`Smoke evidence: ${scratch}`)
}

main().catch(error => { console.error(error); process.exitCode = 1 })
