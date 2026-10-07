import { Box, type DOMElement } from 'ink'
import { useImperativeHandle, useLayoutEffect, useRef, useState, type ReactNode, type Ref } from 'react'

export type TranscriptScroll = {
  scrollBy: (rows: number) => void
  returnToLatest: () => void
}

export function TranscriptViewport({ height, scrollRef, onScrollChange, children }: {
  height: number
  scrollRef: Ref<TranscriptScroll>
  onScrollChange: (scrolled: boolean) => void
  children: ReactNode
}) {
  const contentRef = useRef<DOMElement>(null)
  const [contentHeight, setContentHeight] = useState(0)
  // null follows new output; a row number anchors the history being read.
  const [position, setPosition] = useState<number | null>(null)
  const maxScroll = Math.max(0, contentHeight - height)
  const top = position === null ? maxScroll : Math.min(position, maxScroll)
  const scrolled = position !== null && top < maxScroll

  useLayoutEffect(() => {
    const measured = contentRef.current?.yogaNode?.getComputedHeight() || 0
    setContentHeight(current => current === measured ? current : measured)
  })
  useLayoutEffect(() => {
    onScrollChange(scrolled)
  }, [scrolled, onScrollChange])
  useLayoutEffect(() => {
    if (position !== null && position >= maxScroll) setPosition(null)
  }, [position, maxScroll])
  useImperativeHandle(scrollRef, () => ({
    scrollBy(rows) {
      if (!maxScroll) return
      setPosition(current => {
        const next = Math.max(0, Math.min(maxScroll, (current ?? maxScroll) + rows))
        return next === maxScroll ? null : next
      })
    },
    returnToLatest() { setPosition(null) },
  }), [maxScroll])

  return (
    <Box height={height} flexShrink={0} flexDirection="column" overflowY="hidden">
      <Box ref={contentRef} flexDirection="column" flexShrink={0} marginTop={-top}>
        {children}
      </Box>
    </Box>
  )
}
