import { memo, useMemo, useState } from 'react'
import type { DiffLine } from '../lib/diff'

const COLLAPSE_CONTEXT = 4

/** Unified diff with old/new gutters; long unchanged stretches fold away. */
export const DiffView = memo(function DiffView({ lines, maxLines, compact = false }: { lines: DiffLine[]; maxLines?: number; compact?: boolean }) {
  const [expanded, setExpanded] = useState<Set<number>>(new Set())
  const [showAll, setShowAll] = useState(false)
  const rows = useMemo(() => {
    // Fold runs of context longer than 2*COLLAPSE_CONTEXT+2 lines.
    const result: Array<DiffLine | { type: 'fold'; start: number; count: number }> = []
    let index = 0
    while (index < lines.length) {
      if (lines[index].type !== 'context') { result.push(lines[index]); index++; continue }
      let end = index
      while (end < lines.length && lines[end].type === 'context') end++
      const run = end - index
      const head = index === 0 ? 0 : COLLAPSE_CONTEXT, tail = end === lines.length ? 0 : COLLAPSE_CONTEXT
      if (run > head + tail + 2 && !expanded.has(index)) {
        result.push(...lines.slice(index, index + head))
        result.push({ type: 'fold', start: index, count: run - head - tail })
        result.push(...lines.slice(end - tail, end))
      } else result.push(...lines.slice(index, end))
      index = end
    }
    return result
  }, [lines, expanded])
  const visible = maxLines && !showAll ? rows.slice(0, maxLines) : rows
  if (!lines.length) return <div className="diff-empty">没有可显示的差异</div>
  return <div className={`diff-view ${compact ? 'diff-compact' : ''}`} role="table" aria-label="文件差异">
    {visible.map((line, index) => {
      if (line.type === 'fold') return <button key={`fold-${line.start}`} className="diff-fold" onClick={() => setExpanded(previous => new Set(previous).add(line.start))}>
        展开 {line.count} 行未改动内容
      </button>
      if (line.type === 'meta') return compact ? null : <div key={index} className="diff-row diff-meta" role="row"><span className="diff-text">{line.text}</span></div>
      if (line.type === 'hunk') return <div key={index} className="diff-row diff-hunk" role="row"><span className="diff-gutter" /><span className="diff-gutter" /><span className="diff-text">{line.text}</span></div>
      return <div key={index} role="row" className={`diff-row diff-${line.type}`}>
        <span className="diff-gutter" aria-hidden="true">{line.oldLine ?? ''}</span>
        <span className="diff-gutter" aria-hidden="true">{line.newLine ?? ''}</span>
        <span className="diff-sign" aria-hidden="true">{line.type === 'add' ? '+' : line.type === 'remove' ? '−' : ' '}</span>
        <span className="diff-text">{line.text || ' '}</span>
      </div>
    })}
    {maxLines && rows.length > maxLines && !showAll && <button className="diff-fold" onClick={() => setShowAll(true)}>显示全部 {rows.length} 行</button>}
  </div>
})

export function DiffStat({ additions = 0, deletions = 0 }: { additions?: number; deletions?: number }) {
  if (!additions && !deletions) return null
  return <span className="diff-stat"><span className="diff-stat-add">+{additions}</span><span className="diff-stat-remove">−{deletions}</span></span>
}
