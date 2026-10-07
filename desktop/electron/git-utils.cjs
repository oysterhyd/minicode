// Pure helpers for the main process, kept separate so they can be unit tested
// without Electron.

/** Parse `git diff --numstat -z` output into Map<path, {additions, deletions}>. */
function parseNumstat(stdout) {
  const stats = new Map()
  const fields = String(stdout || '').split('\0')
  for (let index = 0; index < fields.length; index++) {
    const record = fields[index]
    if (!record) continue
    const match = /^(-|\d+)\t(-|\d+)\t(.*)$/s.exec(record)
    if (!match) continue
    let file = match[3]
    // Renames and copies: "a\td\t\0old\0new\0" - the new path is the one status reports.
    if (!file) { index += 2; file = fields[index] }
    if (!file) continue
    const additions = match[1] === '-' ? 0 : Number(match[1])
    const deletions = match[2] === '-' ? 0 : Number(match[2])
    const previous = stats.get(file)
    stats.set(file, previous
      ? { additions: previous.additions + additions, deletions: previous.deletions + deletions }
      : { additions, deletions })
  }
  return stats
}

/** Parse `git status --porcelain=v1 -z` output into [{status, path}]. */
function parseStatus(stdout) {
  const fields = String(stdout || '').split('\0')
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

/** Count text lines; binary content (NUL byte) counts as 0. */
function countLines(buffer) {
  if (!buffer || buffer.length === 0) return 0
  if (buffer.includes(0)) return 0
  let lines = 0
  for (const byte of buffer) if (byte === 10) lines++
  return buffer[buffer.length - 1] === 10 ? lines : lines + 1
}

const EXTERNAL_PROTOCOLS = new Set(['http:', 'https:', 'mailto:'])

function isAllowedExternal(url) {
  if (typeof url !== 'string') return false
  try { return EXTERNAL_PROTOCOLS.has(new URL(url).protocol) } catch { return false }
}

/** Whether navigation stays inside the app (dev server, or the built index.html). */
function isAppUrl(url, devServer = null, appFileUrl = null) {
  if (typeof url !== 'string') return false
  try {
    const parsed = new URL(url)
    if (devServer) return parsed.origin === new URL(devServer).origin && !parsed.username && !parsed.password
    if (!appFileUrl || parsed.protocol !== 'file:' || parsed.host) return false
    const expected = new URL(appFileUrl)
    if (expected.protocol !== 'file:' || expected.host) return false
    const { fileURLToPath } = require('node:url')
    const path = require('node:path')
    const canonical = value => {
      const result = path.resolve(fileURLToPath(value))
      return process.platform === 'win32' ? result.toLowerCase() : result
    }
    return canonical(parsed) === canonical(expected)
  } catch { return false }
}

const RECENT_LIMIT = 12

/** Accept both the legacy `{workspace}` file and the current `{workspace, recent}`. */
function normalizeSettings(raw) {
  const value = raw && typeof raw === 'object' ? raw : {}
  const workspace = typeof value.workspace === 'string' ? value.workspace : null
  const recent = Array.isArray(value.recent) ? value.recent.filter(item => typeof item === 'string') : []
  return { workspace, recent: addRecent(recent, workspace) }
}

function samePath(a, b) {
  return process.platform === 'win32' ? a.toLowerCase() === b.toLowerCase() : a === b
}

function addRecent(recent, workspace) {
  if (!workspace) return recent.slice(0, RECENT_LIMIT)
  return [workspace, ...recent.filter(item => !samePath(item, workspace))].slice(0, RECENT_LIMIT)
}

function removeRecent(recent, workspace) {
  return recent.filter(item => typeof workspace !== 'string' || !samePath(item, workspace))
}

/** Whether a saved window rectangle is still meaningfully visible on some display. */
function boundsVisible(bounds, workAreas) {
  if (!bounds || ![bounds.x, bounds.y, bounds.width, bounds.height].every(Number.isFinite)) return false
  return workAreas.some(area => {
    const width = Math.min(bounds.x + bounds.width, area.x + area.width) - Math.max(bounds.x, area.x)
    const height = Math.min(bounds.y + bounds.height, area.y + area.height) - Math.max(bounds.y, area.y)
    return width >= 120 && height >= 60
  })
}

module.exports = { parseNumstat, parseStatus, countLines, isAllowedExternal, isAppUrl, normalizeSettings, addRecent, removeRecent, boundsVisible, RECENT_LIMIT }
