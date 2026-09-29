export type DiffLine = { type: 'add' | 'remove' | 'context' | 'hunk' | 'meta'; text: string; oldLine?: number; newLine?: number }

/** Parse unified diff text (git or tool output) into renderable lines. */
export function parseUnifiedDiff(text: string): DiffLine[] {
  const lines: DiffLine[] = []
  let oldLine = 0, newLine = 0, inHunk = false
  for (const raw of text.replace(/\r\n/g, '\n').split('\n')) {
    const hunk = raw.match(/^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@(.*)$/)
    if (hunk) {
      oldLine = Number(hunk[1]); newLine = Number(hunk[2]); inHunk = true
      lines.push({ type: 'hunk', text: raw })
      continue
    }
    if (!inHunk || raw.startsWith('diff --git') || raw.startsWith('index ') || raw.startsWith('--- ') || raw.startsWith('+++ ')) {
      if (raw.startsWith('diff --git')) inHunk = false
      if (raw) lines.push({ type: 'meta', text: raw })
      continue
    }
    if (raw.startsWith('+')) lines.push({ type: 'add', text: raw.slice(1), newLine: newLine++ })
    else if (raw.startsWith('-')) lines.push({ type: 'remove', text: raw.slice(1), oldLine: oldLine++ })
    else if (raw.startsWith('\\')) lines.push({ type: 'meta', text: raw })
    else lines.push({ type: 'context', text: raw.slice(1), oldLine: oldLine++, newLine: newLine++ })
  }
  return lines
}

export function looksLikeDiff(text: string) {
  return /^@@ -\d+/m.test(text) && /^[+-]/m.test(text)
}

/** Line diff between two snippets (LCS); falls back to remove/add for large input. */
export function diffTexts(before: string, after: string): DiffLine[] {
  const a = before.replace(/\r\n/g, '\n').split('\n'), b = after.replace(/\r\n/g, '\n').split('\n')
  if (a.length * b.length > 250_000) {
    return [...a.map((text, i) => ({ type: 'remove' as const, text, oldLine: i + 1 })), ...b.map((text, i) => ({ type: 'add' as const, text, newLine: i + 1 }))]
  }
  const table = Array.from({ length: a.length + 1 }, () => new Uint32Array(b.length + 1))
  for (let i = a.length - 1; i >= 0; i--) for (let j = b.length - 1; j >= 0; j--) {
    table[i][j] = a[i] === b[j] ? table[i + 1][j + 1] + 1 : Math.max(table[i + 1][j], table[i][j + 1])
  }
  const lines: DiffLine[] = []
  let i = 0, j = 0
  while (i < a.length || j < b.length) {
    if (i < a.length && j < b.length && a[i] === b[j]) { lines.push({ type: 'context', text: a[i], oldLine: i + 1, newLine: j + 1 }); i++; j++ }
    else if (j < b.length && (i >= a.length || table[i][j + 1] >= table[i + 1][j])) { lines.push({ type: 'add', text: b[j], newLine: j + 1 }); j++ }
    else { lines.push({ type: 'remove', text: a[i], oldLine: i + 1 }); i++ }
  }
  return lines
}

export function diffStats(lines: DiffLine[]) {
  return { additions: lines.filter(line => line.type === 'add').length, deletions: lines.filter(line => line.type === 'remove').length }
}
