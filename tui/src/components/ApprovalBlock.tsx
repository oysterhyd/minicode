import { Box, Text } from 'ink'
import type { ReactNode } from 'react'
import type { FeedItem } from '../types.js'
import { previewDiff, toolMeta, toolTarget, type DiffLine } from '../tools.js'
import { displayWidth, terminalText, truncateWidth } from '../format.js'
import { theme } from '../terminal.js'
import { DiffRows } from './DiffView.js'

const PREVIEW_LINES = 6

// Resolved approvals stay in the transcript as a single quiet line; the open
// request itself is drawn by ApprovalPanel in place of the prompt.
export function ApprovalBlock({ item }: { item: FeedItem }) {
  // Granted requests are evident from the tool row that follows.
  if (item.approvalId || item.granted) return null
  const meta = toolMeta(item.name)
  const target = terminalText(toolTarget(item)).split('\n')[0]
  return (
    <Box marginBottom={1}>
      <Text wrap="truncate-end">
        <Text dimColor>{'  ⎿  '}</Text>
        <Text color={theme.error}>已拒绝</Text>
        <Text dimColor> {meta.done}{target ? `(${target})` : ''}</Text>
      </Text>
    </Box>
  )
}

function title(item: FeedItem): string {
  if (item.name === 'workspace_trust') return '信任工作区插件'
  if (item.name === 'bash') return 'Bash 命令'
  if (item.name === 'powershell') return 'PowerShell 命令'
  if (item.name === 'edit') return '编辑文件'
  if (item.name === 'write') return '写入文件'
  return `工具调用 · ${toolMeta(item.name).done}`
}

export function approvalOptions(item: FeedItem): string[] {
  // Trust is remembered per plugin fingerprint either way; a changed plugin asks again.
  if (item.name === 'workspace_trust') return ['是，信任并启动这些 MCP 服务', '是（插件内容变化前不再询问）', '否，不启动']
  const always = item.name === 'bash' ? '是，并且本会话不再询问 Bash 命令'
    : item.name === 'edit' ? '是，并且本会话不再询问文件编辑'
      : item.name === 'write' ? '是，并且本会话不再询问文件写入'
        : `是，并且本会话不再询问 ${toolMeta(item.name).done}`
  return ['是', always, '否，并告诉 MiniCode 应该怎么做']
}

function previewLines(item: FeedItem): { text: string[]; diff: DiffLine[] } {
  const args = item.args || {}
  if (item.name === 'workspace_trust') return { text: terminalText(item.text || '').split('\n').slice(1), diff: [] }
  if (item.name === 'bash' || item.name === 'powershell') return { text: terminalText(String(args.command || '')).split('\n'), diff: [] }
  if (item.name === 'edit') {
    return { text: [String(args.path || '')], diff: previewDiff(String(args.old_text ?? ''), String(args.new_text ?? '')) }
  }
  if (item.name === 'write') {
    const content = String(args.content ?? '')
    return { text: [String(args.path || '')], diff: previewDiff('', content) }
  }
  const target = toolTarget(item)
  return { text: (target || JSON.stringify(args)).split('\n'), diff: [] }
}

// Every row is a single terminal line so the App can reserve exactly
// approvalRows(...).length rows above the bottom of the screen.
export function approvalRows(item: FeedItem, choice: number, columns: number): ReactNode[] {
  const width = Math.max(1, columns - 1)
  const { text, diff } = previewLines(item)
  const rows: ReactNode[] = []
  rows.push(<Text color={theme.permission}>{'─'.repeat(width)}</Text>)
  rows.push(<Text bold color={theme.permission}> {truncateWidth(title(item), width - 1)}</Text>)
  rows.push(<Text> </Text>)
  const shownText = text.slice(0, PREVIEW_LINES)
  for (const line of shownText) rows.push(<Text>{'   '}{truncateWidth(terminalText(line) || ' ', width - 3)}</Text>)
  if (text.length > shownText.length) rows.push(<Text dimColor>{'   '}… +{text.length - shownText.length} 行</Text>)
  const shownDiff = diff.slice(0, PREVIEW_LINES)
  if (shownDiff.length) rows.push(<DiffRows lines={shownDiff} width={width} indent="   " numbered={false} />)
  for (let extra = 1; extra < shownDiff.length; extra++) rows.push(null)
  if (diff.length > shownDiff.length) rows.push(<Text dimColor>{'   '}… +{diff.length - shownDiff.length} 行</Text>)
  const summary = terminalText(item.text || '').split('\n')[0]
  const generated = summary.startsWith(`${item.name}:`)
  if (summary && !generated && summary !== text[0] && item.name !== 'edit' && item.name !== 'write') {
    rows.push(<Text dimColor>{'   '}{truncateWidth(summary, width - 3)}</Text>)
  }
  rows.push(<Text> </Text>)
  rows.push(<Text> 是否继续？</Text>)
  approvalOptions(item).forEach((option, index) => {
    const active = index === choice
    const label = `${index + 1}. ${option}${index === 2 ? ' (esc)' : ''}`
    rows.push(
      <Text color={active ? theme.permission : undefined}>
        {active ? ' ❯ ' : '   '}{truncateWidth(label, Math.max(1, width - 3))}
      </Text>,
    )
  })
  rows.push(<Text> </Text>)
  const hint = '↑↓ 选择 · enter 确认 · 也可直接按 y / a / n'
  rows.push(<Text dimColor>{' '}{displayWidth(hint) < width ? hint : truncateWidth(hint, width - 1)}</Text>)
  return rows
}

export function ApprovalPanel({ item, choice, columns }: { item: FeedItem; choice: number; columns: number }) {
  const rows = approvalRows(item, choice, columns)
  return (
    <Box flexDirection="column" flexShrink={0}>
      {rows.map((row, index) => row === null ? null : <Box key={index} flexShrink={0} flexDirection="column">{row}</Box>)}
    </Box>
  )
}
