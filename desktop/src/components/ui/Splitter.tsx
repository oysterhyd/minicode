import { useRef, useState } from 'react'

/** Drag handle between panes. `invert` for a pane that sits to the right of the handle. */
export function Splitter({ label, value, min, max, invert = false, onChange }: {
  label: string; value: number; min: number; max: number; invert?: boolean; onChange: (value: number) => void
}) {
  const start = useRef<{ x: number; value: number } | null>(null)
  const [dragging, setDragging] = useState(false)
  const clamp = (next: number) => Math.round(Math.min(max, Math.max(min, next)))
  return <div role="separator" aria-orientation="vertical" aria-label={label} aria-valuenow={value} aria-valuemin={min} aria-valuemax={max} tabIndex={0}
    className={`splitter ${dragging ? 'is-dragging' : ''}`}
    onPointerDown={event => {
      event.preventDefault()
      event.currentTarget.setPointerCapture(event.pointerId)
      start.current = { x: event.clientX, value }; setDragging(true)
      document.documentElement.classList.add('is-resizing')
    }}
    onPointerMove={event => {
      if (!start.current) return
      const delta = event.clientX - start.current.x
      onChange(clamp(start.current.value + (invert ? -delta : delta)))
    }}
    onPointerUp={() => { start.current = null; setDragging(false); document.documentElement.classList.remove('is-resizing') }}
    onPointerCancel={() => { start.current = null; setDragging(false); document.documentElement.classList.remove('is-resizing') }}
    onDoubleClick={() => onChange(clamp(invert ? 400 : 268))}
    onKeyDown={event => {
      if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return
      event.preventDefault()
      const step = event.shiftKey ? 40 : 12
      onChange(clamp(value + ((event.key === 'ArrowRight') !== invert ? step : -step)))
    }} />
}
