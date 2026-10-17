import { Box, Text } from 'ink'
import { useEffect, useState } from 'react'
import { displayWidth, formatTokens, terminalText, truncateWidth } from '../format.js'
import { theme, useTerminalSize } from '../terminal.js'
import type { AgentState } from '../types.js'

const EFFORT_LABELS: Record<string, string> = { low: '低', medium: '中', high: '高', xhigh: '超高', max: '最大' }

export const NEXT_PERMISSION: Record<string, string> = { default: 'accept_edits', accept_edits: 'default', bypass: 'default' }

function contextLabel(state: AgentState, columns: number): string {
  if (!state.contextWindow) return `${formatTokens(state.contextTokens)} tokens`
  const percent = Math.min(100, Math.round(state.contextTokens / state.contextWindow * 100))
  return columns < 40 ? `${percent}%` : `上下文 ${percent}%`
}

// The footer under the prompt: mode hint on the left, model and context on
// the right. The model always survives narrow terminals; the hint goes first.
export function StatusLine({ state }: { state: AgentState }) {
  const { columns } = useTerminalSize()
  const effort = state.effort && state.effort !== 'off' ? ` · ${EFFORT_LABELS[state.effort] || state.effort}` : ''
  const suffix = `${effort} · ${contextLabel(state, columns)}`
  const width = Math.max(0, columns - 2)
  const model = truncateWidth(terminalText(state.model || '未选择模型'), Math.max(4, width - displayWidth(suffix)))
  const right = `${model}${suffix}`
  const mode = state.permissionMode
  const pending = state.taskPending && !state.running ? ' · /continue 继续任务' : ''
  const left = mode === 'accept_edits' ? `⏵⏵ 自动接受编辑${pending}`
    : mode === 'bypass' ? `⏵⏵ 全部允许${pending}` : `? 查看快捷键${pending}`
  const hint = mode === 'default' ? '' : ' (shift+tab 切换)'
  const room = width - displayWidth(right) - 2
  const showLeft = room >= displayWidth(left)
  const showHint = showLeft && room >= displayWidth(left + hint)
  const color = mode === 'accept_edits' ? theme.autoAccept : mode === 'bypass' ? theme.bypass : undefined
  return (
    <Box paddingX={1} justifyContent="space-between" width={columns} flexShrink={0}>
      {showLeft ? (
        <Text wrap="truncate-end">
          <Text color={color} dimColor={!color}>{left}</Text>
          {showHint ? <Text dimColor>{hint}</Text> : null}
        </Text>
      ) : <Text> </Text>}
      <Text dimColor wrap="truncate-end">{right}</Text>
    </Box>
  )
}

const SHORTCUTS: Array<[string, string]> = [
  ['/', '命令'],
  ['enter', '发送 / 排队'],
  ['ctrl+j', '换行'],
  ['shift+tab', '切换权限模式'],
  ['ctrl+o', '展开工具输出'],
  ['esc', '取消 / 拒绝'],
  ['ctrl+c', '中断 / 退出'],
  ['pgup/pgdn', '翻阅历史'],
  ['ctrl+end', '回到最新'],
]

export function shortcutLayout(columns: number): { perRow: number; rows: number } {
  const perRow = columns >= 84 ? 3 : columns >= 52 ? 2 : 1
  return { perRow, rows: Math.ceil(SHORTCUTS.length / perRow) }
}

export function ShortcutPanel() {
  const { columns } = useTerminalSize()
  const { perRow, rows } = shortcutLayout(columns)
  const cell = Math.floor((columns - 2) / perRow)
  return (
    <Box flexDirection="column" paddingX={1} flexShrink={0}>
      {Array.from({ length: rows }, (_, row) => (
        <Text key={row} wrap="truncate-end">
          {SHORTCUTS.slice(row * perRow, row * perRow + perRow).map(([key, detail], index) => {
            const text = `${key} ${detail}`
            const pad = Math.max(0, cell - displayWidth(text))
            return (
              <Text key={index}>
                <Text color={theme.claude}>{key}</Text>
                <Text dimColor> {truncateWidth(detail, Math.max(0, cell - displayWidth(key) - 1))}{index < perRow - 1 ? ' '.repeat(pad) : ''}</Text>
              </Text>
            )
          })}
        </Text>
      ))}
    </Box>
  )
}

// ✳ renders as a colour emoji outside macOS terminals, as in Claude Code.
const FRAMES = ['·', '✢', process.platform === 'darwin' ? '✳' : '*', '✶', '✻', '✽']
const CYCLE = [...FRAMES, ...FRAMES.slice(1, -1).reverse()]
const VERBS = ['思考中', '琢磨中', '推敲中', '构思中', '酝酿中', '梳理中', '斟酌中', '钻研中']
const TIPS = [
  'Enter 可以把下一条消息排进队列',
  'shift+tab 切换自动接受编辑',
  'ctrl+j 在输入框中换行',
  'PageUp 翻阅更早的输出',
  '输入 /model 随时切换模型',
]

// Self-timed so the shimmer can run faster than the transcript re-renders.
export function Spinner({ startedAt, tokens, hint }: { startedAt: number; now?: number; tokens?: number; hint?: boolean }) {
  const [tick, setTick] = useState(0)
  useEffect(() => {
    const timer = setInterval(() => setTick(value => value + 1), 120)
    return () => clearInterval(timer)
  }, [])
  const { columns } = useTerminalSize()
  const now = Date.now()
  const seconds = Math.max(0, Math.floor((now - startedAt) / 1000))
  const elapsed = seconds >= 60 ? `${Math.floor(seconds / 60)}m ${seconds % 60}s` : `${seconds}s`
  const verb = `${VERBS[Math.abs(Math.floor(startedAt / 1000)) % VERBS.length]}…`
  const chars = [...verb]
  const sweep = (tick % (chars.length + 8)) - 4
  const usage = tokens ? ` · ↓ ${formatTokens(tokens)} tokens` : ''
  const detail = `(${elapsed}${usage} · ctrl+c 中断)`
  const tip = hint ? 'ctrl+o 展开最近的工具输出' : TIPS[Math.abs(Math.floor(startedAt / 1000)) % TIPS.length]
  return (
    <Box marginBottom={1} flexDirection="column" flexShrink={0}>
      <Text wrap="truncate-end">
        <Text color={theme.claude}>{CYCLE[tick % CYCLE.length]} </Text>
        {chars.map((char, index) => (
          <Text key={index} color={Math.abs(index - sweep) <= 1 ? theme.shimmer : theme.claude}>{char}</Text>
        ))}
        <Text dimColor> {truncateWidth(detail, Math.max(0, columns - 4 - displayWidth(verb)))}</Text>
      </Text>
      <Text dimColor wrap="truncate-end">{'  ⎿  提示：'}{tip}</Text>
    </Box>
  )
}

export function Notice({ text, tone }: { text: string; tone?: 'info' | 'warning' | 'error' }) {
  const color = tone === 'error' ? theme.error : tone === 'warning' ? theme.warning : undefined
  return (
    <Box marginBottom={1}>
      <Text color={color} dimColor={!color}>{terminalText(text)}</Text>
    </Box>
  )
}
