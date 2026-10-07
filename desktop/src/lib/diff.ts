export type DiffLine = { type: 'add' | 'remove' | 'context' | 'hunk' | 'meta'; text: string; oldLine?: number; newLine?: number }

/** Parse unified diff text (git or tool output) into renderable lines. */
export function parseUnifiedDiff(text: string): DiffLine[] {
  const lines: DiffLine[] = []
  let oldLine = 0, newLine = 0, oldRemaining = 0, newRemaining = 0
  for (const raw of text.replace(/\r\n/g, '\n').split('\n')) {
    const hunk = raw.match(/^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@(.*)$/)
    if (hunk) {
      oldLine = Number(hunk[1]); newLine = Number(hunk[3])
      oldRemaining = hunk[2] === undefined ? 1 : Number(hunk[2])
      newRemaining = hunk[4] === undefined ? 1 : Number(hunk[4])
      lines.push({ type: 'hunk', text: raw })
      continue
    }
    if (raw.startsWith('+') && newRemaining > 0) {
      lines.push({ type: 'add', text: raw.slice(1), newLine: newLine++ }); newRemaining--
    } else if (raw.startsWith('-') && oldRemaining > 0) {
      lines.push({ type: 'remove', text: raw.slice(1), oldLine: oldLine++ }); oldRemaining--
    } else if (raw.startsWith(' ') && oldRemaining > 0 && newRemaining > 0) {
      lines.push({ type: 'context', text: raw.slice(1), oldLine: oldLine++, newLine: newLine++ })
      oldRemaining--; newRemaining--
    } else if (raw) {
      if (!raw.startsWith('\\')) oldRemaining = newRemaining = 0
      lines.push({ type: 'meta', text: raw })
    }
  }
  return lines
}

export function looksLikeDiff(text: string) {
  return /^@@ -\d+/m.test(text) && /^[+-]/m.test(text)
}

/** Bounded Myers shortest-edit path. Typical similar files cost O((N+M)D),
 * with O(D²) compact typed traceback; pathological inputs have a hard work cap. */
function myers(a: string[], b: string[], start: number, aEnd: number, bEnd: number): DiffLine[] | null {
  const n = aEnd - start, m = bEnd - start
  const trace: Int32Array[] = []
  let work = 0, cells = 0
  const get = (row: Int32Array, diagonal: number, depth: number) => row[diagonal + depth] ?? -1
  for (let depth = 0; depth <= n + m; depth++) {
    cells += 2 * depth + 1
    if (cells > 250_000) return null
    const row = new Int32Array(2 * depth + 1)
    const previous = trace[depth - 1]
    for (let diagonal = -depth; diagonal <= depth; diagonal += 2) {
      if (++work > 250_000) return null
      let x = depth === 0 ? 0 : diagonal === -depth || (diagonal !== depth && get(previous, diagonal - 1, depth - 1) < get(previous, diagonal + 1, depth - 1))
        ? get(previous, diagonal + 1, depth - 1) : get(previous, diagonal - 1, depth - 1) + 1
      let y = x - diagonal
      while (x < n && y < m && a[start + x] === b[start + y]) {
        if (++work > 250_000) return null
        x++; y++
      }
      row[diagonal + depth] = x
      if (x >= n && y >= m) {
        trace.push(row)
        const result: DiffLine[] = []
        for (let d = depth; d > 0; d--) {
          const k = x - y, prev = trace[d - 1]
          const down = k === -d || (k !== d && get(prev, k - 1, d - 1) < get(prev, k + 1, d - 1))
          const prevK = down ? k + 1 : k - 1
          const prevX = get(prev, prevK, d - 1), prevY = prevX - prevK
          while (x > prevX && y > prevY) {
            result.push({ type: 'context', text: a[start + x - 1], oldLine: start + x, newLine: start + y }); x--; y--
          }
          if (down) { result.push({ type: 'add', text: b[start + y - 1], newLine: start + y }); y-- }
          else { result.push({ type: 'remove', text: a[start + x - 1], oldLine: start + x }); x-- }
        }
        while (x > 0 && y > 0) {
          result.push({ type: 'context', text: a[start + x - 1], oldLine: start + x, newLine: start + y }); x--; y--
        }
        return result.reverse()
      }
    }
    trace.push(row)
  }
  return null
}

/** Trim equal edges, then diff only the changed middle; no N×M table. */
export function diffTexts(before: string, after: string): DiffLine[] {
  const a = before === '' ? [] : before.replace(/\r\n/g, '\n').split('\n')
  const b = after === '' ? [] : after.replace(/\r\n/g, '\n').split('\n')
  const lines: DiffLine[] = []
  let start = 0, aEnd = a.length, bEnd = b.length
  while (start < aEnd && start < bEnd && a[start] === b[start]) {
    lines.push({ type: 'context', text: a[start], oldLine: start + 1, newLine: start + 1 }); start++
  }
  while (aEnd > start && bEnd > start && a[aEnd - 1] === b[bEnd - 1]) { aEnd--; bEnd-- }
  const middle = start === aEnd || start === bEnd ? null : myers(a, b, start, aEnd, bEnd)
  if (middle) for (const line of middle) lines.push(line)
  else {
    for (let i = start; i < aEnd; i++) lines.push({ type: 'remove', text: a[i], oldLine: i + 1 })
    for (let j = start; j < bEnd; j++) lines.push({ type: 'add', text: b[j], newLine: j + 1 })
  }
  for (let i = aEnd, j = bEnd; i < a.length; i++, j++) lines.push({ type: 'context', text: a[i], oldLine: i + 1, newLine: j + 1 })
  return lines
}

export function diffStats(lines: DiffLine[]) {
  let additions = 0, deletions = 0
  for (const line of lines) {
    if (line.type === 'add') additions++
    else if (line.type === 'remove') deletions++
  }
  return { additions, deletions }
}
