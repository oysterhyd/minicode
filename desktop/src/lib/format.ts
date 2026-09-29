export function basename(path: string | null | undefined) {
  if (!path) return ''
  return path.split(/[\\/]/).filter(Boolean).pop() || path
}

export function dirname(path: string) {
  const index = path.lastIndexOf('/')
  return index > 0 ? path.slice(0, index) : ''
}

export function compactNumber(value: number) {
  return new Intl.NumberFormat('en-US', { notation: 'compact', maximumFractionDigits: 1 }).format(value)
}

export function duration(ms: number) {
  if (!Number.isFinite(ms) || ms < 0) return ''
  if (ms < 10_000) return `${Math.max(0.1, ms / 1000).toFixed(1)}s`
  const seconds = Math.round(ms / 1000)
  if (seconds < 60) return `${seconds}s`
  const minutes = Math.floor(seconds / 60)
  if (minutes < 60) return `${minutes}m ${String(seconds % 60).padStart(2, '0')}s`
  return `${Math.floor(minutes / 60)}h ${minutes % 60}m`
}

export function clock(seconds: number) {
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`
}

export function relativeTime(value: string | undefined, now = Date.now()) {
  const date = new Date(value || '')
  const time = date.getTime()
  if (Number.isNaN(time)) return ''
  const diff = Math.max(0, now - time)
  const minute = 60_000, hour = 60 * minute, day = 24 * hour
  if (diff < minute) return '刚刚'
  if (diff < hour) return `${Math.floor(diff / minute)} 分钟`
  if (diff < day) return `${Math.floor(diff / hour)} 小时`
  if (diff < 7 * day) return `${Math.floor(diff / day)} 天`
  return `${date.getMonth() + 1}/${date.getDate()}`
}

export type DateBucket = 'today' | 'yesterday' | 'week' | 'month' | 'older'
export const bucketLabels: Record<DateBucket, string> = { today: '今天', yesterday: '昨天', week: '近 7 天', month: '近 30 天', older: '更早' }

export function dateBucket(value: string | undefined, now = new Date()): DateBucket {
  const time = new Date(value || '').getTime()
  if (Number.isNaN(time)) return 'older'
  const start = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime()
  const day = 86_400_000
  if (time >= start) return 'today'
  if (time >= start - day) return 'yesterday'
  if (time >= start - 6 * day) return 'week'
  if (time >= start - 29 * day) return 'month'
  return 'older'
}

/** Subsequence fuzzy match. Returns a score (higher is better) or -1. */
/** Rank a path, preferring basename hits. Returns -1 when neither matches. */
export function pathScore(path: string, query: string) {
  const name = fuzzyScore(path.split('/').pop() || path, query)
  const full = fuzzyScore(path, query)
  if (name < 0 && full < 0) return -1
  return Math.max(name < 0 ? -1 : name + 50, full)
}

export function fuzzyScore(text: string, query: string) {
  if (!query) return 0
  const haystack = text.toLowerCase(), needle = query.toLowerCase()
  const direct = haystack.indexOf(needle)
  if (direct >= 0) return 1000 - direct - (haystack.length - needle.length) * 0.01
  let score = 0, position = -1, streak = 0
  for (const char of needle) {
    const next = haystack.indexOf(char, position + 1)
    if (next < 0) return -1
    streak = next === position + 1 ? streak + 1 : 0
    score += 10 + streak * 5 - Math.min(9, next - position - 1)
    position = next
  }
  return score
}

export const isMac = typeof navigator !== 'undefined' && /Mac/i.test(navigator.platform)
export const modKey = isMac ? '⌘' : 'Ctrl'
