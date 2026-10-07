import { Box, Text, useCursor, useStdout, type DOMElement } from 'ink'
import { useLayoutEffect, useRef, useState, type RefObject } from 'react'
import { displayWidth, truncateWidth, terminalText } from '../format.js'
import { promptRows, takeChar } from '../caret.js'
import { ACCENT, useTerminalSize } from '../terminal.js'
import type { Suggestion } from '../slash.js'

export function SlashMenu({ items, selected, title }: { items: Suggestion[]; selected: number; title?: string }) {
  const { columns, rows } = useTerminalSize()
  const count = Math.max(1, Math.min(8, Math.floor(rows / 3)))
  const first = Math.max(0, Math.min(selected - count + 1, items.length - count))
  const valueWidth = Math.max(8, Math.floor(columns / 2))
  return (
    <Box flexDirection="column" marginBottom={1} paddingX={1}>
      <Text dimColor>{title || '命令'} · ↑↓ 选择 · Tab 补全 · Enter 执行</Text>
      {items.length ? items.slice(first, first + count).map((item, offset) => (
        <Text key={`${item.kind}-${item.value}`}>
          <Text color={ACCENT}>{first + offset === selected ? '❯ ' : '  '}</Text>
          <Text bold={first + offset === selected}>{truncateWidth(terminalText(item.value), valueWidth)}</Text>
          <Text dimColor>  {truncateWidth(terminalText(item.detail), Math.max(0, columns - 6 - Math.min(displayWidth(terminalText(item.value)), valueWidth)))}</Text>
        </Text>
      )) : <Text dimColor>没有匹配项</Text>}
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

export function Prompt({ value, caret, busy, queued, waiting }: {
  value: string; caret: number; busy: boolean; queued?: string; waiting?: boolean
}) {
  const lineRef = useRef<DOMElement>(null)
  const { columns, rows: terminalRows } = useTerminalSize()
  const { rows, row, column } = promptRows(value, caret, columns - 4)
  const count = Math.max(1, Math.min(6, Math.floor(terminalRows / 3)))
  const first = Math.max(0, row - count + 1)
  return (
    <Box flexDirection="column">
      {queued ? <Text dimColor>  {truncateWidth(queued, columns - 2)}</Text> : null}
      <Box paddingX={1} flexDirection="column" borderStyle="single" borderColor="gray" borderLeft={false} borderRight={false}>
        <Box ref={lineRef} flexDirection="column">
          {rows.slice(first, first + count).map(({ text }, offset) => {
            const active = first + offset === row
            const before = active ? text.slice(0, column) : text
            const [head, tail] = takeChar(active ? text.slice(column) : '')
            return (
              <Text key={first + offset} wrap="truncate-end">
                <Text color={ACCENT}>{first + offset === 0 ? '❯ ' : '  '}</Text>
                {before}
                {active ? <Text>{head || ' '}</Text> : null}
                {active ? tail : null}
                {!value ? <Text dimColor>{waiting ? 'Y 允许 · A 记住 · N 拒绝' : busy ? '输入下一条消息…' : '输入任务，/ 查看命令'}</Text> : null}
              </Text>
            )
          })}
        </Box>
      </Box>
      <PromptCursor targetRef={lineRef} x={2 + displayWidth(rows[row].text.slice(0, column))} y={row - first} />
    </Box>
  )
}
