import { Text } from 'ink'
import { displayWidth, graphemes, terminalText } from '../format.js'
import { theme } from '../terminal.js'
import { diffPairs, wordDiff, type DiffLine } from '../tools.js'

type Segment = { text: string; strong?: boolean }

function fit(segments: Segment[], max: number): { segments: Segment[]; width: number } {
  const out: Segment[] = []
  let width = 0
  for (const segment of segments) {
    let text = ''
    for (const { segment: char } of graphemes.segment(segment.text)) {
      const next = displayWidth(char)
      if (width + next > max) {
        if (text) out.push({ ...segment, text })
        return { segments: out, width }
      }
      text += char
      width += next
    }
    if (text) out.push({ ...segment, text })
  }
  return { segments: out, width }
}

// One terminal row per diff line: line number, +/- marker and the code on a
// full-width red or green band, with the changed words in a stronger tone.
export function DiffRows({ lines, width, indent = '', numbered = true }: {
  lines: DiffLine[]
  width: number
  indent?: string
  numbered?: boolean
}) {
  const pairs = diffPairs(lines)
  const digits = numbered ? Math.max(1, ...lines.map(line => String(line.line ?? '').length)) : 0
  return (
    <>
      {lines.map((line, index) => {
        const text = terminalText(line.text)
        const marker = line.kind === 'add' ? '+' : line.kind === 'del' ? '-' : ' '
        const prefix = `${numbered ? `${String(line.line ?? '').padStart(digits)} ` : ''}${marker} `
        const available = Math.max(0, width - displayWidth(indent) - displayWidth(prefix))
        let segments: Segment[] = [{ text }]
        const partner = pairs.get(index)
        if (partner !== undefined && line.kind !== 'context') {
          const other = terminalText(lines[partner].text)
          const words = line.kind === 'del' ? wordDiff(text, other) : wordDiff(other, text)
          if (words) {
            segments = (line.kind === 'del' ? words.before : words.after)
              .map(part => ({ text: part.text, strong: part.changed && Boolean(part.text.trim()) }))
          }
        }
        const fitted = fit(segments, available)
        if (line.kind === 'context') {
          return (
            <Text key={index} wrap="truncate-end">
              {indent}<Text dimColor>{prefix}</Text>{fitted.segments.map(segment => segment.text).join('') || ' '}
            </Text>
          )
        }
        const band = line.kind === 'add' ? theme.addBg : theme.delBg
        const word = line.kind === 'add' ? theme.addWord : theme.delWord
        return (
          <Text key={index} wrap="truncate-end">
            {indent}
            <Text backgroundColor={band}>{prefix}</Text>
            {fitted.segments.map((segment, part) => (
              <Text key={part} backgroundColor={segment.strong ? word : band}>{segment.text}</Text>
            ))}
            <Text backgroundColor={band}>{' '.repeat(Math.max(0, available - fitted.width))}</Text>
          </Text>
        )
      })}
    </>
  )
}
