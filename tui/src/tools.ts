import type { FeedItem } from './types.js'

type ToolMeta = { verb: string; done: string }

const meta: Record<string, ToolMeta> = {
  read: { verb: '读取', done: 'Read' },
  read_artifact: { verb: '读取归档', done: 'Read' },
  ls: { verb: '列出', done: 'List' },
  grep: { verb: '搜索', done: 'Grep' },
  edit: { verb: '编辑', done: 'Edit' },
  write: { verb: '写入', done: 'Write' },
  bash: { verb: '运行', done: 'Bash' },
  delegate: { verb: '子助手', done: 'Delegate' },
  skills_list: { verb: '查看技能', done: 'Skills' },
  skill_load: { verb: '加载技能', done: 'Skill' },
  skill_unload: { verb: '卸载技能', done: 'Skill' },
  skill_resource: { verb: '读取技能资源', done: 'Skill' },
  memory_list: { verb: '查看记忆', done: 'Memory' },
  task_create: { verb: '创建任务', done: 'Task' },
  task_list: { verb: '查看任务', done: 'Task' },
  task_claim: { verb: '认领任务', done: 'Task' },
  task_complete: { verb: '完成任务', done: 'Task' },
}

export function toolMeta(name = ''): ToolMeta {
  if (meta[name]) return meta[name]
  if (name.startsWith('mcp__')) return { verb: '调用', done: name.split('__').slice(1).join(' · ') }
  return { verb: '调用', done: name || 'Tool' }
}

export function toolTarget(item: Pick<FeedItem, 'name' | 'args'>): string {
  const args = item.args || {}
  const value = args.path ?? args.file_path ?? args.command ?? args.pattern ?? args.task ?? args.title ?? args.name ?? args.kind ?? args.task_id
  if (item.name === 'grep' && args.pattern) return `${args.pattern}${args.path && args.path !== '.' ? `  in ${args.path}` : ''}`
  if (item.name === 'ls' && !args.path) return '.'
  return value === undefined ? '' : String(value)
}

export function toolMark(item: FeedItem): string {
  if (item.pending) return '◌'
  if (item.interrupted) return '◼'
  if (item.success === false) return '✗'
  return '✓'
}

export type DiffLine = { kind: 'context' | 'add' | 'del'; text: string; line?: number }
export type ParsedDiff = { path: string; adds: number; dels: number; lines: DiffLine[] }

// edit/write report "Applied change to <path> (+N -M)" followed by a unified
// diff. Parse it so the transcript can show Claude Code style numbered hunks.
export function parseToolDiff(output = ''): ParsedDiff | null {
  const text = output.replace(/\r\n/g, '\n')
  const head = /^Applied (?:change|write) to (.+?) \(\+(\d+) -(\d+)\)/.exec(text)
  if (!head) return null
  const lines: DiffLine[] = []
  let oldLine = 0
  let newLine = 0
  let inHunk = false
  for (const row of text.split('\n').slice(1)) {
    const hunk = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(row)
    if (hunk) {
      if (lines.length) lines.push({ kind: 'context', text: '…' })
      oldLine = Number(hunk[1])
      newLine = Number(hunk[2])
      inHunk = true
      continue
    }
    if (!inHunk || row.startsWith('\\')) continue
    if (row.startsWith('+')) lines.push({ kind: 'add', text: row.slice(1), line: newLine++ })
    else if (row.startsWith('-')) lines.push({ kind: 'del', text: row.slice(1), line: oldLine++ })
    else if (row.startsWith(' ')) {
      lines.push({ kind: 'context', text: row.slice(1), line: newLine })
      oldLine++
      newLine++
    }
  }
  while (lines.at(-1)?.kind === 'context' && !lines.at(-1)?.text) lines.pop()
  return { path: head[1], adds: Number(head[2]), dels: Number(head[3]), lines }
}

// A compact before/after preview for approvals, where no unified diff exists
// yet. Shared leading and trailing lines become context.
export function previewDiff(before = '', after = ''): DiffLine[] {
  const split = (text: string) => text ? text.replace(/\r\n/g, '\n').replace(/\n$/, '').split('\n') : []
  const a = split(before)
  const b = split(after)
  let start = 0
  while (start < a.length && start < b.length && a[start] === b[start]) start++
  let end = 0
  while (end < a.length - start && end < b.length - start && a[a.length - 1 - end] === b[b.length - 1 - end]) end++
  return [
    ...a.slice(Math.max(0, start - 1), start).map(text => ({ kind: 'context' as const, text })),
    ...a.slice(start, a.length - end).map(text => ({ kind: 'del' as const, text })),
    ...b.slice(start, b.length - end).map(text => ({ kind: 'add' as const, text })),
    ...a.slice(a.length - end, a.length - end + 1).map(text => ({ kind: 'context' as const, text })),
  ]
}

export type WordSegment = { text: string; changed: boolean }

function tokens(text: string): string[] {
  return text.match(/[\p{L}\p{N}_]+|\s+|[^\p{L}\p{N}_\s]/gu) || []
}

function merge(parts: WordSegment[]): WordSegment[] {
  const out: WordSegment[] = []
  for (const part of parts) {
    const last = out.at(-1)
    if (last && last.changed === part.changed) last.text += part.text
    else out.push({ ...part })
  }
  return out
}

// Word-level diff of a removed line against its replacement (LCS over word,
// space and punctuation tokens). Lines that changed almost entirely return
// null so they keep the plain band, as Claude Code does.
export function wordDiff(before: string, after: string): { before: WordSegment[]; after: WordSegment[] } | null {
  const a = tokens(before)
  const b = tokens(after)
  if (!a.length || !b.length || a.length * b.length > 40_000) return null
  const table = Array.from({ length: a.length + 1 }, () => new Uint16Array(b.length + 1))
  for (let i = a.length - 1; i >= 0; i--) {
    for (let j = b.length - 1; j >= 0; j--) {
      table[i][j] = a[i] === b[j] ? table[i + 1][j + 1] + 1 : Math.max(table[i + 1][j], table[i][j + 1])
    }
  }
  const left: WordSegment[] = []
  const right: WordSegment[] = []
  let i = 0
  let j = 0
  while (i < a.length || j < b.length) {
    if (i < a.length && j < b.length && a[i] === b[j]) {
      left.push({ text: a[i++], changed: false })
      right.push({ text: b[j++], changed: false })
    } else if (j < b.length && (i >= a.length || table[i][j + 1] >= table[i + 1][j])) {
      right.push({ text: b[j++], changed: true })
    } else {
      left.push({ text: a[i++], changed: true })
    }
  }
  const changed = (parts: WordSegment[]) => parts.filter(part => part.changed && part.text.trim()).reduce((sum, part) => sum + part.text.length, 0)
  const ratio = Math.max(changed(left) / Math.max(1, before.trim().length), changed(right) / Math.max(1, after.trim().length))
  if (ratio > 0.6) return null
  return { before: merge(left), after: merge(right) }
}

export function diffPairs(lines: DiffLine[]): Map<number, number> {
  const pairs = new Map<number, number>()
  for (let index = 0; index < lines.length;) {
    if (lines[index].kind !== 'del') { index++; continue }
    let dels = index
    while (dels < lines.length && lines[dels].kind === 'del') dels++
    let adds = dels
    while (adds < lines.length && lines[adds].kind === 'add') adds++
    if (adds > dels) {
      for (let offset = 0; offset < Math.min(dels - index, adds - dels); offset++) {
        pairs.set(index + offset, dels + offset)
        pairs.set(dels + offset, index + offset)
      }
    }
    index = adds
  }
  return pairs
}
