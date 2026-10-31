import { Box, renderToString, useStdout } from 'ink'
import { useEffect, useRef, type ReactNode } from 'react'

// Write sealed rows above Ink's live region using public APIs. Unlike Static,
// this does not retain a second copy for resize replay after /cls.
export function TranscriptLog<T>({ items, epoch, columns, renderItem }: {
  items: T[]; epoch: number; columns: number; renderItem: (item: T, index: number) => ReactNode
}) {
  const { write } = useStdout()
  const cursor = useRef({ epoch, count: 0 })
  useEffect(() => {
    if (cursor.current.epoch !== epoch || items.length < cursor.current.count) cursor.current = { epoch, count: 0 }
    const start = cursor.current.count
    if (start === items.length) return
    const pending = items.slice(start)
    cursor.current.count = items.length
    // A separate renderer must run after React has finished this commit.
    queueMicrotask(() => {
      const output = renderToString(<Box flexDirection="column">{pending.map((item, i) => <Box key={i} flexDirection="column">{renderItem(item, start + i)}</Box>)}</Box>, { columns })
      write(output + '\n')
    })
  }, [items, epoch, columns, renderItem, write])
  return null
}
