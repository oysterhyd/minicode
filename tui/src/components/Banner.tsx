import os from 'node:os'
import { Box, Text } from 'ink'
import { displayWidth, terminalText, truncateWidth } from '../format.js'
import { theme, useTerminalSize } from '../terminal.js'

// A small pixel "M" in the spirit of Claude Code's welcome mascot.
const MASCOT = [' ▗▄  ▄▖ ', ' ▐█▙▟█▌ ', ' ▐▌▝▘▐▌ ']

const TIPS: Array<[string, string]> = [
  ['/help', '查看全部命令与快捷键'],
  ['/model', '切换模型，/resume 恢复会话'],
  ['shift+tab', '切换权限模式'],
]

export type BannerProps = {
  version: string
  workspace: string
  model: string
  permissionMode?: string
  recent?: string[]
}

function userName(): string {
  try {
    return os.userInfo().username || ''
  } catch {
    return ''
  }
}

export function homePath(workspace: string): string {
  const home = os.homedir()
  if (home && (workspace === home || workspace.startsWith(`${home}/`) || workspace.startsWith(`${home}\\`))) {
    return `~${workspace.slice(home.length)}`
  }
  return workspace
}

function TopEdge({ width, version }: { width: number; version: string }) {
  const title = ` MiniCode v${version} `
  const lead = width >= displayWidth(title) + 6 ? '╭───' : '╭'
  const fill = Math.max(0, width - displayWidth(lead) - displayWidth(title) - 1)
  return (
    <Text>
      <Text color={theme.claude}>{lead}</Text>
      <Text color={theme.claude} bold>{truncateWidth(title, Math.max(0, width - 2))}</Text>
      <Text color={theme.claude}>{'─'.repeat(fill)}╮</Text>
    </Text>
  )
}

export function Banner({ version, workspace, model, recent = [] }: BannerProps) {
  const { columns } = useTerminalSize()
  const width = Math.max(4, columns)
  const name = userName()
  const welcome = name ? `欢迎回来，${name}！` : '欢迎使用 MiniCode！'
  const where = terminalText(homePath(workspace))
  const modelName = terminalText(model || '选择模型后开始')
  const inner = width - 4

  if (width < 70) {
    return (
      <Box flexDirection="column" marginBottom={1} flexShrink={0}>
        <TopEdge width={width} version={version} />
        <Box flexDirection="column" borderStyle="round" borderColor={theme.claude} borderTop={false} paddingX={1} width={width}>
          <Text bold wrap="truncate-end">{welcome}</Text>
          <Text dimColor wrap="truncate-end">{truncateWidth(`${modelName} · ${where}`, inner)}</Text>
          <Text wrap="truncate-end"><Text color={theme.claude}>/help</Text><Text dimColor> 查看命令 · </Text><Text color={theme.claude}>/resume</Text><Text dimColor> 恢复会话</Text></Text>
        </Box>
      </Box>
    )
  }

  const left = Math.floor(inner * 0.46)
  const right = inner - left - 3
  const sessions = recent.filter(Boolean).slice(0, 3)
  return (
    <Box flexDirection="column" marginBottom={1} flexShrink={0}>
      <TopEdge width={width} version={version} />
      <Box borderStyle="round" borderColor={theme.claude} borderTop={false} paddingX={1} width={width}>
        <Box flexDirection="column" width={left} alignItems="center" flexShrink={0}>
          <Text> </Text>
          <Text bold>{truncateWidth(welcome, left)}</Text>
          <Text> </Text>
          {MASCOT.map((line, index) => <Text key={index} color={theme.claude}>{line}</Text>)}
          <Text> </Text>
          <Text dimColor>{truncateWidth(modelName, left)}</Text>
          <Text dimColor>{truncateWidth(where, left)}</Text>
        </Box>
        <Box flexDirection="column" width={right + 2} flexShrink={0} paddingLeft={1}
          borderStyle="single" borderColor={theme.claude} borderTop={false} borderBottom={false} borderRight={false}>
          <Text color={theme.claude} bold>快速上手</Text>
          {TIPS.map(([key, detail]) => (
            <Text key={key} wrap="truncate-end">{key} <Text dimColor>{truncateWidth(detail, Math.max(0, right - displayWidth(key) - 1))}</Text></Text>
          ))}
          <Text color={theme.claude}>{'─'.repeat(Math.max(0, right))}</Text>
          <Text color={theme.claude} bold>最近活动</Text>
          {sessions.length
            ? sessions.map((title, index) => <Text key={index} wrap="truncate-end">{truncateWidth(terminalText(title).split('\n')[0], right)}</Text>)
            : <Text dimColor>暂无最近会话</Text>}
          <Text dimColor wrap="truncate-end">/resume 查看更多</Text>
        </Box>
      </Box>
    </Box>
  )
}
