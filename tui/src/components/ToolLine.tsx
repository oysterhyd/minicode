import { Box, Text } from 'ink'
import type { ReactNode } from 'react'
import type { FeedItem } from '../types.js'
import type { ThoughtItem } from '../transcript.js'
import { formatElapsed, truncateWidth, terminalText, displayWidth } from '../format.js'
import { theme, useTerminalSize } from '../terminal.js'
import { thoughtLabel, toolOutputLines } from '../transcript.js'
import { parseToolDiff, toolMeta, toolTarget } from '../tools.js'
import { DiffRows } from './DiffView.js'

const COLLAPSED = 3
const EXPANDED = 80
const DIFF_COLLAPSED = 12
const BRANCH = '  ⎿  '
const INDENT = '     '

export function ThoughtLine({ item, now }: { item: ThoughtItem; now: number }) {
  const span = item.startedAt == null ? 0 : ((item.endedAt ?? now) - item.startedAt) / 1000
  const elapsed = span >= 1 ? formatElapsed(item.startedAt, item.endedAt, now) : ''
  const detail = thoughtLabel(item)
  return (
    <Box marginBottom={1}>
      <Text wrap="truncate-end">
        <Text color={theme.success}>● </Text>
        <Text>{terminalText(detail)}</Text>
        {elapsed ? <Text dimColor>  {elapsed}</Text> : null}
      </Text>
    </Box>
  )
}

// Completed rows stay clean like Claude Code; only running tools show time.
function seconds(item: FeedItem, now: number): string {
  if (item.startedAt == null || !item.pending) return ''
  const span = ((item.endedAt ?? now) - item.startedAt) / 1000
  return span >= 1 ? formatElapsed(item.startedAt, item.endedAt, now) : ''
}

function OutputRows({ lines, columns, color }: { lines: string[]; columns: number; color?: string }) {
  return (
    <>
      {lines.map((line, index) => (
        <Text key={index} wrap="truncate-end">
          <Text dimColor>{index === 0 ? BRANCH : INDENT}</Text>
          <Text color={color} dimColor={!color}>{truncateWidth(terminalText(line) || ' ', Math.max(1, columns - 5))}</Text>
        </Text>
      ))}
    </>
  )
}

function More({ hidden, expanded }: { hidden: number; expanded: boolean }) {
  if (!hidden) return null
  return <Text dimColor wrap="truncate-end">{INDENT}… +{hidden} 行{expanded ? '' : ' (ctrl+o 展开)'}</Text>
}

export function ToolLine({
  item,
  now,
  expanded = false,
  detailOnly = false,
}: {
  item: FeedItem
  now: number
  expanded?: boolean
  detailOnly?: boolean
}) {
  const meta = toolMeta(item.name)
  const { columns } = useTerminalSize()
  const target = terminalText(toolTarget(item))
  const failed = item.success === false || item.interrupted
  const skill = Boolean(item.name?.startsWith('skill'))
  const name = skill ? 'Skill' : meta.done
  const argument = skill ? String(item.args?.name || item.args?.skill || target || '') : target
  const dot = item.pending
    ? (Math.floor(now / 500) % 2 ? ' ' : '●')
    : '●'
  const dotColor = item.pending ? theme.subtle : failed ? theme.error : theme.success
  const elapsed = seconds(item, now)
  const nameWidth = displayWidth(name) + 4 + (elapsed ? displayWidth(elapsed) + 2 : 0) + (item.auto ? 9 : 0)
  const raw = toolOutputLines(item)
  const diff = !item.pending && !failed && (item.name === 'edit' || item.name === 'write') ? parseToolDiff(item.output) : null
  const limit = expanded ? EXPANDED : COLLAPSED

  let body: ReactNode = null
  if (item.pending) {
    body = <Text dimColor wrap="truncate-end">{BRANCH}{meta.verb}中…</Text>
  } else if (item.interrupted) {
    body = <OutputRows lines={['已中断']} columns={columns} color={theme.error} />
  } else if (diff) {
    const verb = item.name === 'write' ? '已写入' : '已更新'
    const rows = expanded ? diff.lines.slice(0, EXPANDED) : diff.lines.slice(0, DIFF_COLLAPSED)
    body = (
      <>
        <Text wrap="truncate-end">
          <Text dimColor>{BRANCH}</Text>
          {verb} <Text bold>{terminalText(diff.path)}</Text>，新增 <Text bold>{diff.adds}</Text> 行，删除 <Text bold>{diff.dels}</Text> 行
        </Text>
        <DiffRows lines={rows} width={columns} indent={INDENT} />
        <More hidden={diff.lines.length - rows.length} expanded={expanded} />
      </>
    )
  } else if (raw.length) {
    const shown = raw.slice(0, limit)
    body = (
      <>
        <OutputRows lines={shown} columns={columns} color={failed ? theme.error : undefined} />
        <More hidden={raw.length - shown.length} expanded={expanded} />
      </>
    )
  } else if (failed) {
    body = <OutputRows lines={['失败']} columns={columns} color={theme.error} />
  }

  return (
    <Box flexDirection="column" marginBottom={1}>
      {detailOnly ? null : (
        <Text wrap="truncate-end">
          <Text color={dotColor}>{dot} </Text>
          <Text bold>{terminalText(name)}</Text>
          {argument ? <Text>({truncateWidth(terminalText(argument), Math.max(4, columns - nameWidth))})</Text> : null}
          {elapsed ? <Text dimColor>  {elapsed}</Text> : null}
          {item.auto ? <Text dimColor> · 已自动批准</Text> : null}
        </Text>
      )}
      {body}
    </Box>
  )
}
