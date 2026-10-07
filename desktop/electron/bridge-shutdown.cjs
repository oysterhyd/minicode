'use strict'
const { execFile } = require('node:child_process')
const { setTimeout, clearTimeout } = require('node:timers')

/** Kill descendants before their owner, never just the Python parent. */
function killBridgeTree(child) {
  if (!child.pid) { child.kill(); return Promise.resolve() }
  if (process.platform === 'win32') {
    return new Promise((resolve) => {
      execFile('taskkill', ['/PID', String(child.pid), '/T', '/F'], { windowsHide: true, timeout: 3000 }, () => resolve())
    })
  }
  return new Promise((resolve) => {
    execFile('ps', ['-A', '-o', 'pid=,ppid='], { timeout: 3000 }, (_error, stdout = '') => {
      const children = new Map()
      for (const line of stdout.trim().split('\n')) {
        const [pid, parent] = line.trim().split(/\s+/).map(Number)
        if (!Number.isInteger(pid) || !Number.isInteger(parent)) continue
        if (!children.has(parent)) children.set(parent, [])
        children.get(parent).push(pid)
      }
      const descendants = [child.pid]
      const seen = new Set(descendants)
      for (let i = 0; i < descendants.length; i++) for (const pid of children.get(descendants[i]) || []) {
        if (!seen.has(pid)) { seen.add(pid); descendants.push(pid) }
      }
      for (const pid of descendants.reverse()) {
        try { process.kill(pid, 'SIGKILL') } catch { /* already exited */ }
      }
      resolve()
    })
  })
}

/** EOF gives the bridge a chance to cancel runs, persist state and close MCP. */
async function stopBridge(child, { timeoutMs = 5000, terminateTree = killBridgeTree } = {}) {
  if (!child || child.exitCode !== null || child.signalCode) return
  let timer
  let onExit
  const exited = new Promise(resolve => {
    onExit = () => resolve(true)
    child.once('exit', onExit)
  })
  const deadline = new Promise(resolve => { timer = setTimeout(() => resolve(false), timeoutMs) })
  try {
    try { child.stdin.end() } catch { /* wait for exit or fall back to tree termination */ }
    if (!await Promise.race([exited, deadline])) await terminateTree(child)
  } finally {
    clearTimeout(timer)
    child.removeListener('exit', onExit)
  }
}

module.exports = { stopBridge, killBridgeTree }
