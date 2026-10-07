export function formatTokens(value: number): string {
  const sign = value < 0 ? '-' : ''
  const magnitude = Math.abs(value)
  if (magnitude < 1000) return `${sign}${magnitude}`
  if (magnitude < 999_950) {
    const scaled = magnitude / 1000
    return `${sign}${trimDecimal(scaled)}k`
  }
  return `${sign}${trimDecimal(magnitude / 1_000_000)}M`
}

function trimDecimal(number: number): string {
  return number.toFixed(1).replace(/\.0$/, '')
}

export const PERMISSION_LABELS: Record<string, string> = {
  default: '默认权限',
  accept_edits: '自动接受编辑',
  bypass: '全部允许',
}

export function permissionLabel(mode: string): string {
  return PERMISSION_LABELS[mode] || mode
}

export function formatElapsed(startedAt?: number, endedAt?: number, now = Date.now()): string {
  if (startedAt == null) return ''
  const start = startedAt
  const end = endedAt ?? now
  const seconds = Math.max(0, (end - start) / 1000)
  if (seconds < 10) return `${seconds.toFixed(1)}s`
  return `${Math.round(seconds)}s`
}

export function workspaceName(workspace: string): string {
  const trimmed = workspace.replace(/[\\/]+$/, '')
  const parts = trimmed.split(/[\\/]/)
  return parts.at(-1) || trimmed || workspace
}

export function truncateWidth(text: string, max: number): string {
  if (max <= 0) return ''
  if (displayWidth(text) <= max) return text
  let width = 0
  let result = ''
  for (const { segment: char } of graphemes.segment(text)) {
    const next = displayWidth(char)
    if (width + next > max - 1) break
    result += char
    width += next
  }
  return `${result}…`
}

export function displayWidth(text: string): number {
  return stringWidth(text)
}
import stringWidth from 'string-width'
import stripAnsi from 'strip-ansi'

export const graphemes = new Intl.Segmenter(undefined, { granularity: 'grapheme' })

export function terminalText(text: string, tabWidth = 2): string {
  return stripAnsi(text).replace(/\r\n?/g, '\n').replace(/[\x00-\x08\x0b-\x1f\x7f-\x9f]/g, '').replace(/\t/g, ' '.repeat(tabWidth))
}
