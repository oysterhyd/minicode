import { useEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'

type Tip = { text: string; shortcut?: string; x: number; y: number; placement: 'top' | 'bottom' }

/**
 * One tooltip layer for the whole app. Any element with `data-tip` (and an
 * optional `data-shortcut`) gets a styled tooltip that is never clipped by
 * scroll containers, without wrapping every button in a component.
 */
export function TooltipLayer() {
  const [tip, setTip] = useState<Tip | null>(null)
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const current = useRef<Element | null>(null)
  const shownAt = useRef(0)
  useEffect(() => {
    const hide = () => { if (timer.current) clearTimeout(timer.current); current.current = null; setTip(null) }
    const show = (target: Element) => {
      const text = target.getAttribute('data-tip')
      if (!text) return
      const rect = target.getBoundingClientRect()
      const placement = target.getAttribute('data-tip-placement') === 'bottom' || rect.top < 48 ? 'bottom' : 'top'
      shownAt.current = Date.now()
      setTip({ text, shortcut: target.getAttribute('data-shortcut') || undefined, x: rect.left + rect.width / 2,
        y: placement === 'top' ? rect.top - 8 : rect.bottom + 8, placement })
    }
    const over = (event: PointerEvent) => {
      const target = (event.target as Element | null)?.closest?.('[data-tip]')
      if (target === current.current) return
      if (timer.current) clearTimeout(timer.current)
      current.current = target || null
      if (!target) { setTip(null); return }
      // Moving between toolbar buttons keeps the tooltip responsive.
      const warm = Date.now() - shownAt.current < 600
      timer.current = setTimeout(() => { if (current.current === target && target.isConnected) show(target) }, warm ? 60 : 480)
    }
    const focus = (event: FocusEvent) => {
      const target = (event.target as Element | null)?.closest?.('[data-tip]')
      if (target && (target as HTMLElement).matches(':focus-visible')) { current.current = target; show(target) }
    }
    document.addEventListener('pointerover', over)
    document.addEventListener('pointerdown', hide, true)
    document.addEventListener('focusin', focus)
    document.addEventListener('focusout', hide)
    window.addEventListener('blur', hide)
    window.addEventListener('scroll', hide, true)
    return () => {
      document.removeEventListener('pointerover', over)
      document.removeEventListener('pointerdown', hide, true)
      document.removeEventListener('focusin', focus)
      document.removeEventListener('focusout', hide)
      window.removeEventListener('blur', hide)
      window.removeEventListener('scroll', hide, true)
    }
  }, [])
  if (!tip) return null
  const left = Math.min(window.innerWidth - 12, Math.max(12, tip.x))
  return createPortal(<div className={`tooltip tooltip-${tip.placement}`} role="tooltip" style={{ left, top: tip.y }}>
    <span>{tip.text}</span>{tip.shortcut && <kbd>{tip.shortcut}</kbd>}
  </div>, document.body)
}
