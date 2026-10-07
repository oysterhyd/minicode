import { Box, Text } from 'ink'
import type { FeedItem } from '../types.js'
import type { ThoughtItem } from '../transcript.js'
import { formatElapsed, truncateWidth, terminalText } from '../format.js'
import { useTerminalSize } from '../terminal.js'
import { thoughtLabel, toolOutputLines } from '../transcript.js'
import { toolMark, toolMeta, toolTarget } from '../tools.js'

const COLLAPSED = 6
const EXPANDED = 80

export function ThoughtLine({ item, now }: { item: ThoughtItem; now: number }) {
  const elapsed = formatElapsed(item.startedAt, item.endedAt, now)
  const detail = thoughtLabel(item)
  return (
    <Box paddingLeft={2} marginBottom={1}>
      <Text dimColor>Thought for {elapsed || '0s'}{detail ? `, ${terminalText(detail)}` : ''}</Text>
    </Box>
  )
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
  const elapsed = formatElapsed(item.startedAt, item.endedAt, now)
  const color = item.pending ? 'yellow' : item.success === false || item.interrupted ? 'red' : 'green'
  const skill = Boolean(item.name?.startsWith('skill'))
  const title = skill
    ? `Skill(${String(item.args?.name || item.args?.skill || target || '')})`
    : `${item.pending ? meta.verb : meta.done}${target ? `(${truncateWidth(target, Math.max(4, columns - 24))})` : ''}`
  const raw = toolOutputLines(item)
  const alwaysChild = skill && !item.pending
  const showTree = expanded || alwaysChild || item.success === false || item.interrupted
  const limit = expanded ? EXPANDED : COLLAPSED
  const shown = showTree ? raw.slice(0, limit) : []
  const hidden = showTree ? Math.max(0, raw.length - shown.length) : 0
  const rows = hidden ? [...shown, `… +${hidden} 行`] : shown

  return (
    <Box flexDirection="column" paddingLeft={2}>
      {detailOnly ? null : (
        <Box>
          <Text color={color}>{skill && item.success === true ? '● ' : `${toolMark(item)} `}</Text>
          <Text color={color}>{terminalText(title)}</Text>
          {elapsed ? <Text dimColor>  {elapsed}</Text> : null}
          {item.auto ? <Text color="yellow">  自动</Text> : null}
        </Box>
      )}
      {rows.map((line, index) => (
        <Text key={index} dimColor>
          {'  ⎿ '}
          {terminalText(line) || ' '}
        </Text>
      ))}
    </Box>
  )
}
