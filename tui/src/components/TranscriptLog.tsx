import { Box, renderToString, useStdout } from 'ink'
import { useEffect, useRef, type ReactNode } from 'react'

// Write sealed rows above Ink's live region using public APIs. Unlike Static,
// this does not retain a second copy for resize replay after /cls.
export function TranscriptLog<T extends object>({ items, epoch, columns, renderItem }: {
  items: T[]; epoch: number; columns: number; renderItem: (item: T, index: number) => ReactNode
}) {
  const { write } = useStdout()
  const cursor = useRef({ epoch, printed: new Map<string, T>() })
  useEffect(() => {
    if (cursor.current.epoch !== epoch) cursor.current = { epoch, printed: new Map() }
    const pending: Array<{ item: T; index: number }> = []
    const seen = new Set<string>()
    items.forEach((item, index) => {
      const row = item as Record<string, unknown>
      const key = typeof row.id === 'string' ? row.id : `${row.kind || 'row'}-${index}`
      seen.add(key)
      const previous = cursor.current.printed.get(key)
      if (!previous || previous !== item && (Object.keys(previous).length !== Object.keys(item).length || Object.keys(item).some(field => row[field] !== (previous as Record<string, unknown>)[field]))) pending.push({ item, index })
      cursor.current.printed.set(key, item)
    })
    for (const key of cursor.current.printed.keys()) if (!seen.has(key)) cursor.current.printed.delete(key)
    if (!pending.length) return
    // A separate renderer must run after React has finished this commit.
    queueMicrotask(() => {
      if (cursor.current.epoch !== epoch) return
      const output = renderToString(<Box flexDirection="column">{pending.map(({ item, index }) => <Box key={index} flexDirection="column">{renderItem(item, index)}</Box>)}</Box>, { columns })
      write(output + '\n')
    })
  }, [items, epoch, columns, renderItem, write])
  return null
}
