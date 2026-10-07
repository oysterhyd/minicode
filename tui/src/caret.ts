import { displayWidth, graphemes } from './format.js'

export function clamp(index: number, text: string): number {
  const at = Math.max(0, Math.min(index, text.length))
  if (at === 0 || at === text.length) return at
  for (const part of graphemes.segment(text)) {
    if (part.index === at) return at
    if (part.index + part.segment.length > at) return part.index
  }
  return at
}

export function prevBoundary(text: string, index: number): number {
  const at = clamp(index, text)
  let previous = 0
  for (const part of graphemes.segment(text)) {
    if (part.index >= at) break
    previous = part.index
  }
  return previous
}

export function nextBoundary(text: string, index: number): number {
  const at = clamp(index, text)
  for (const part of graphemes.segment(text)) {
    if (part.index >= at) return part.index + part.segment.length
  }
  return text.length
}

export function insertAt(text: string, index: number, chunk: string): string {
  const at = clamp(index, text)
  return text.slice(0, at) + chunk + text.slice(at)
}

export function deleteBackward(text: string, index: number): { text: string; caret: number } {
  const at = clamp(index, text)
  const from = prevBoundary(text, at)
  return { text: text.slice(0, from) + text.slice(at), caret: from }
}

export function deleteForward(text: string, index: number): { text: string; caret: number } {
  const at = clamp(index, text)
  const to = nextBoundary(text, at)
  return { text: text.slice(0, at) + text.slice(to), caret: at }
}

export function caretCoords(text: string, index: number): { line: number; column: number; lines: string[] } {
  const at = clamp(index, text)
  const lines = text.split('\n')
  let remaining = at
  for (let line = 0; line < lines.length; line++) {
    const length = lines[line].length
    if (remaining <= length) return { line, column: remaining, lines }
    remaining -= length + 1
  }
  const last = lines.length - 1
  return { line: last, column: lines[last].length, lines }
}

export function indexAt(lines: string[], line: number, column: number): number {
  const row = Math.max(0, Math.min(line, lines.length - 1))
  let index = 0
  for (let i = 0; i < row; i++) index += lines[i].length + 1
  return index + Math.max(0, Math.min(column, lines[row].length))
}

export function moveVertical(text: string, index: number, delta: number): number {
  const { line, column, lines } = caretCoords(text, index)
  const row = Math.max(0, Math.min(line + delta, lines.length - 1))
  const desired = displayWidth(lines[line].slice(0, column))
  let width = 0
  let target = 0
  for (const { segment } of graphemes.segment(lines[row])) {
    if (width + displayWidth(segment) > desired) break
    width += displayWidth(segment)
    target += segment.length
  }
  return indexAt(lines, row, target)
}

export function lineStart(text: string, index: number): number {
  const { line, lines } = caretCoords(text, index)
  return indexAt(lines, line, 0)
}

export function lineEnd(text: string, index: number): number {
  const { line, lines } = caretCoords(text, index)
  return indexAt(lines, line, lines[line].length)
}

export function prevWord(text: string, index: number): number {
  let at = clamp(index, text)
  while (at > 0 && /\s/.test(text[at - 1] || '')) at -= 1
  while (at > 0 && !/\s/.test(text[at - 1] || '')) at -= 1
  return at
}

export function nextWord(text: string, index: number): number {
  let at = clamp(index, text)
  while (at < text.length && !/\s/.test(text[at] || '')) at += 1
  while (at < text.length && /\s/.test(text[at] || '')) at += 1
  return at
}

export function takeChar(text: string): [string, string] {
  if (!text) return ['', '']
  const end = nextBoundary(text, 0)
  return [text.slice(0, end), text.slice(end)]
}

/** Hard-wrap on terminal cells; each offset stays on a grapheme boundary. */
export function promptRows(text: string, index: number, columns: number) {
  const width = Math.max(2, columns)
  const rows: Array<{ text: string; start: number }> = [{ text: '', start: 0 }]
  let cells = 0
  for (const { segment, index: start } of graphemes.segment(text)) {
    if (segment === '\n') {
      rows.push({ text: '', start: start + 1 })
      cells = 0
      continue
    }
    const size = displayWidth(segment)
    if (cells + size > width) {
      rows.push({ text: '', start })
      cells = 0
    }
    rows.at(-1)!.text += segment
    cells += size
  }
  const at = clamp(index, text)
  let row = rows.findLastIndex(item => item.start <= at)
  let column = at - rows[row].start
  // Reserve a cell for the cursor at the end of a full visual row.
  if (column === rows[row].text.length && displayWidth(rows[row].text) === width) {
    rows.splice(row + 1, 0, { text: '', start: at })
    row += 1
    column = 0
  }
  return { rows, row, column }
}
