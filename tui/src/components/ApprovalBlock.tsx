import { Box, Text } from 'ink'
import type { FeedItem } from '../types.js'
import { toolMeta, toolTarget } from '../tools.js'
import { terminalText } from '../format.js'

function preview(item: FeedItem): string {
  const args = item.args || {}
  if (item.name === 'bash') return String(args.command || '')
  if (args.path) return String(args.path)
  const target = toolTarget(item)
  return target || JSON.stringify(args)
}

export function ApprovalBlock({ item }: { item: FeedItem }) {
  const meta = toolMeta(item.name)
  const title = item.name === 'bash' ? '运行终端命令' : item.name === 'edit' ? '编辑文件' : item.name === 'write' ? '写入文件' : `调用 ${meta.done}`
  if (!item.approvalId) {
    return (
      <Box paddingLeft={2} marginY={1}>
        <Text color={item.granted ? 'green' : 'red'}>{item.granted ? '已批准' : '已拒绝'} · {meta.done} {terminalText(toolTarget(item))}</Text>
      </Box>
    )
  }
  return (
    <Box flexDirection="column" marginY={1} paddingX={1} borderStyle="round" borderColor="yellow">
      <Text bold color="yellow">需要你的批准 · {terminalText(title)}</Text>
      <Text>{terminalText(preview(item))}</Text>
      <Text dimColor>Y 允许 · A 本会话记住 · N 拒绝 · Esc 拒绝（输入为空时）</Text>
    </Box>
  )
}
