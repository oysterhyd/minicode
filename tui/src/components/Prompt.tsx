import { Box, Text, useCursor, useStdout, type DOMElement } from 'ink'
import { useLayoutEffect, useRef, useState, type RefObject } from 'react'
import { displayWidth, truncateWidth, terminalText } from '../format.js'
import { promptRows, takeChar } from '../caret.js'
import { theme, useTerminalSize } from '../terminal.js'
import type { Suggestion } from '../slash.js'

export function slashMenuRows(rows: number): number {
  return Math.max(1, Math.min(10, Math.floor(rows / 3)))
}

// Claude Code lists commands under the input: a fixed command column and a
// dim description, with the highlighted row in the permission blue.
export function SlashMenu({ items, selected, title }: { items: Suggestion[]; selected: number; title?: string }) {
  const { columns, rows } = useTerminalSize()
  const count = slashMenuRows(rows)
  const first = Math.max(0, Math.min(selected - count + 1, items.length - count))
  const shown = items.slice(first, first + count)
  const valueWidth = Math.min(Math.max(8, Math.floor(columns / 2.5)),
    Math.max(8, ...shown.map(item => displayWidth(terminalText(item.value)))))
  return (
    <Box flexDirection="column" paddingLeft={2} flexShrink={0}>
      {title ? <Text dimColor wrap="truncate-end">{title} · ↑↓ 选择 · Tab 补全 · Enter 确认</Text> : null}
      {items.length ? shown.map((item, offset) => {
        const active = first + offset === selected
        const value = truncateWidth(terminalText(item.value), valueWidth)
        const pad = ' '.repeat(Math.max(0, valueWidth - displayWidth(value)) + 3)
        return (
          <Text key={`${item.kind}-${item.value}`} wrap="truncate-end">
            <Text color={active ? theme.permission : undefined} bold={active}>{value}</Text>
            <Text color={active ? theme.permission : undefined} dimColor={!active}>{pad}{truncateWidth(terminalText(item.detail), Math.max(0, columns - 6 - valueWidth))}</Text>
          </Text>
        )
      }) : <Text dimColor>没有匹配项</Text>}
    </Box>
  )
}

function PromptCursor({ x, y, targetRef }: { x: number; y: number; targetRef: RefObject<DOMElement | null> }) {
  const { setCursorPosition } = useCursor()
  const { stdout } = useStdout()
  const [position, setPosition] = useState<{ x: number; y: number }>()
  setCursorPosition(position)
  useLayoutEffect(() => {
    let left = x
    let top = y
    let node = targetRef.current
    let outputHeight = 0
    if (!node?.yogaNode) return
    while (node?.yogaNode) {
      left += node.yogaNode.getComputedLeft()
      top += node.yogaNode.getComputedTop()
      outputHeight = node.yogaNode.getComputedHeight()
      node = node.parentNode || null
    }
    // Ink 6.8 omits the final newline in fullscreen output, but its cursor
    // suffix still assumes that newline moved the terminal down one row.
    if (stdout.isTTY && stdout.rows && outputHeight >= stdout.rows) top += 1
    setPosition(current => current?.x === left && current.y === top ? current : { x: left, y: top })
  })
  return null
}

export const IDLE_HINT = '输入任务，或试试 "解释这个项目是怎么组织的"'

export function Prompt({ value, caret, busy, queued }: {
  value: string; caret: number; busy: boolean; queued?: string; waiting?: boolean
}) {
  const lineRef = useRef<DOMElement>(null)
  const { columns, rows: terminalRows } = useTerminalSize()
  const { rows, row, column } = promptRows(value, caret, columns - 4)
  const count = Math.max(1, Math.min(6, Math.floor(terminalRows / 3)))
  const first = Math.max(0, row - count + 1)
  return (
    <Box flexDirection="column" flexShrink={0}>
      {queued ? <Text dimColor wrap="truncate-end">{truncateWidth(queued, columns)}</Text> : null}
      <Box paddingX={1} flexDirection="column" borderStyle="single" borderColor={theme.rule} borderLeft={false} borderRight={false}>
        <Box ref={lineRef} flexDirection="column">
          {rows.slice(first, first + count).map(({ text }, offset) => {
            const active = first + offset === row
            const before = active ? text.slice(0, column) : text
            const [head, tail] = takeChar(active ? text.slice(column) : '')
            return (
              <Text key={first + offset} wrap="truncate-end">
                <Text color={theme.subtle}>{first + offset === 0 ? '> ' : '  '}</Text>
                {before}
                {active ? <Text>{head || ' '}</Text> : null}
                {active ? tail : null}
                {!value ? <Text dimColor>{busy ? '输入下一条消息…' : IDLE_HINT}</Text> : null}
              </Text>
            )
          })}
        </Box>
      </Box>
      <PromptCursor targetRef={lineRef} x={2 + displayWidth(rows[row].text.slice(0, column))} y={row - first} />
    </Box>
  )
}
