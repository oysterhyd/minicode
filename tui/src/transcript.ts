import type { FeedItem } from './types.js'
import { isLive } from './feed.js'

export type ThoughtItem = {
  kind: 'thought'
  id: string
  reads: number
  searches: number
  startedAt?: number
  endedAt?: number
  target?: string
}

export type LayoutItem = (FeedItem | ThoughtItem) & { lead?: boolean }

const QUIET = new Set(['read', 'read_artifact', 'ls', 'grep'])

export function isQuietTool(name = ''): boolean {
  return QUIET.has(name)
}

export function thoughtLabel(item: ThoughtItem): string {
  const parts: string[] = []
  if (item.reads === 1 && item.target) parts.push(`读取 ${item.target}`)
  else if (item.reads) parts.push(`读取 ${item.reads} 个文件`)
  if (item.searches) parts.push(`搜索 ${item.searches} 次`)
  return parts.join(' · ') || '收集上下文'
}

function thoughtFrom(tools: FeedItem[]): ThoughtItem {
  const first = tools[0]
  const last = tools.at(-1)!
  let reads = 0
  let searches = 0
  for (const item of tools) {
    if (item.name === 'grep') searches += 1
    else reads += 1
  }
  return {
    kind: 'thought',
    id: `thought-${first.id}`,
    reads,
    searches,
    startedAt: first.startedAt,
    endedAt: Math.max(...tools.map(item => item.endedAt || 0)) || last.endedAt,
    target: reads + searches === 1 ? String(first.args?.path || first.args?.pattern || '') : undefined,
  }
}

export function layoutTranscript(items: FeedItem[], settled = true): { frozen: LayoutItem[]; live: LayoutItem[] } {
  const rows: LayoutItem[] = []
  let barrier = Infinity
  let quiet: FeedItem[] = []

  function flushQuiet(closed: boolean) {
    if (!quiet.length) return
    // Static is append-only. A group cannot freeze until no more tools can join
    // it, and nothing after an unfinished row may overtake that row.
    if (!closed || quiet.some(item => item.pending)) barrier = Math.min(barrier, rows.length)
    if (quiet.some(item => item.pending)) rows.push(...quiet)
    else rows.push(thoughtFrom(quiet))
    quiet = []
  }

  for (const item of items) {
    if (item.kind === 'tool' && isQuietTool(item.name) && !item.interrupted
      && (item.pending || item.success === true)) {
      quiet.push(item)
      continue
    }
    flushQuiet(true)
    if (isLive(item)) barrier = Math.min(barrier, rows.length)
    // Keep the entire streaming Markdown document live. Later chunks can change
    // headings, lists and reference links even before an earlier blank line.
    rows.push(item)
  }
  flushQuiet(settled)
  return { frozen: rows.slice(0, barrier), live: rows.slice(barrier) }
}

export function lastExpandableTool(items: FeedItem[], afterUserIndex: number): FeedItem | undefined {
  return [...items].reverse().find(item => (
    item.kind === 'tool'
    && items.indexOf(item) >= afterUserIndex
    && Boolean(item.output || item.args?.command || item.name?.startsWith('skill'))
  ))
}

export function toolOutputLines(item: FeedItem): string[] {
  if (item.name === 'skill_load' && item.success === true) return [item.output || 'Successfully loaded skill']
  const raw = String(item.output || '').replace(/\r\n/g, '\n').replace(/\r/g, '\n')
  return raw.split('\n').filter((line, index, lines) => line.length > 0 || index < lines.length - 1)
}

export function groupTranscript(items: FeedItem[]): LayoutItem[] {
  const layout = layoutTranscript(items)
  return [...layout.frozen, ...layout.live]
}
