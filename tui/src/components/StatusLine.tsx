import { Box, Text } from 'ink'
import { displayWidth, formatTokens, permissionLabel, terminalText, truncateWidth } from '../format.js'
import { ACCENT, useTerminalSize } from '../terminal.js'
import type { AgentState } from '../types.js'

export function StatusLine({ state }: { state: AgentState }) {
  const ctx = `${formatTokens(state.contextTokens)}/${formatTokens(state.contextWindow)}`
  const { columns } = useTerminalSize()
  const permission = columns < 64
    ? ({ default: '需审批', accept_edits: '编辑允许', bypass: '全允许' }[state.permissionMode] || permissionLabel(state.permissionMode))
    : permissionLabel(state.permissionMode)
  const suffix = `${state.effort !== 'off' ? ` · ${state.effort}` : ''} · ${permission} · ${columns < 40 ? '' : 'ctx '}${ctx}${state.taskPending && !state.running ? ' · /continue' : ''}`
  return (
    <Box paddingX={1}>
      <Text dimColor wrap="truncate-end">
        {truncateWidth(terminalText(state.model || '未选择模型'), Math.max(4, columns - 2 - displayWidth(suffix)))}
        {suffix}
      </Text>
    </Box>
  )
}

export function Spinner({ startedAt, now, tokens, hint }: { startedAt: number; now: number; tokens?: number; hint?: boolean }) {
  const seconds = Math.max(0, (now - startedAt) / 1000)
  const label = seconds < 10 ? seconds.toFixed(1) : String(Math.round(seconds))
  const usage = tokens ? ` · ↓ ${formatTokens(tokens)}` : ''
  return (
    <Box marginY={1} flexDirection="column">
      <Text color={ACCENT}>{['✻', '✶', '✳', '✢'][Math.floor(now / 400) % 4]} 正在处理… <Text dimColor>({label}s{usage})</Text></Text>
      <Text dimColor>  {hint ? 'Ctrl+O 展开工具 · ' : ''}Ctrl+C 中断 · Ctrl+J 换行</Text>
    </Box>
  )
}

export function Notice({ text, tone }: { text: string; tone?: 'info' | 'warning' | 'error' }) {
  const color = tone === 'error' ? 'red' : tone === 'warning' ? 'yellow' : undefined
  return (
    <Box marginY={1}>
      <Text color={color} dimColor={!color}>{terminalText(text)}</Text>
    </Box>
  )
}
