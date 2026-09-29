import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from 'react'
import { createPortal } from 'react-dom'
import { motion } from 'motion/react'
import type { LucideIcon } from 'lucide-react'

export type MenuItem = { label: string; icon?: LucideIcon; shortcut?: string; danger?: boolean; disabled?: boolean; run: () => void } | 'divider'

/** Floating menu anchored to a point (context menu) or an element (overflow button). */
export function Menu({ items, anchor, onClose, label }: {
  items: MenuItem[]; anchor: { x: number; y: number } | HTMLElement; onClose: () => void; label: string
}) {
  const root = useRef<HTMLDivElement>(null)
  const [position, setPosition] = useState<{ left: number; top: number } | null>(null)
  const close = useRef(onClose)
  close.current = onClose
  useLayoutEffect(() => {
    const menu = root.current!
    const point = anchor instanceof HTMLElement ? (() => { const r = anchor.getBoundingClientRect(); return { x: r.right - menu.offsetWidth, y: r.bottom + 4 } })() : anchor
    setPosition({ left: Math.max(8, Math.min(point.x, window.innerWidth - menu.offsetWidth - 8)),
      top: point.y + menu.offsetHeight > window.innerHeight - 8 ? Math.max(8, point.y - menu.offsetHeight - (anchor instanceof HTMLElement ? anchor.offsetHeight + 8 : 0)) : point.y })
    menu.querySelector<HTMLElement>('[role="menuitem"]:not(:disabled)')?.focus()
  }, [anchor])
  useEffect(() => {
    const down = (event: PointerEvent) => { if (!root.current?.contains(event.target as Node)) close.current() }
    const key = (event: KeyboardEvent) => {
      if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); close.current(); if (anchor instanceof HTMLElement) anchor.focus() }
      if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return
      event.preventDefault()
      const buttons = Array.from(root.current!.querySelectorAll<HTMLButtonElement>('[role="menuitem"]:not(:disabled)'))
      const index = buttons.indexOf(document.activeElement as HTMLButtonElement)
      buttons[event.key === 'Home' ? 0 : event.key === 'End' ? buttons.length - 1 : (index + (event.key === 'ArrowDown' ? 1 : -1) + buttons.length) % buttons.length]?.focus()
    }
    document.addEventListener('pointerdown', down, true)
    document.addEventListener('keydown', key, true)
    window.addEventListener('blur', close.current)
    window.addEventListener('resize', close.current)
    return () => {
      document.removeEventListener('pointerdown', down, true)
      document.removeEventListener('keydown', key, true)
      window.removeEventListener('blur', close.current)
      window.removeEventListener('resize', close.current)
    }
  }, [anchor])
  return createPortal(<motion.div ref={root} role="menu" aria-label={label} className="floating-menu" style={position || { left: -9999, top: -9999 }}
    initial={{ opacity: 0, scale: .96, y: -4 }} animate={{ opacity: 1, scale: 1, y: 0 }} transition={{ duration: .12, ease: [.2, .8, .2, 1] }}>
    {items.map((item, index) => item === 'divider' ? <div key={`divider-${index}`} className="menu-divider" role="separator" /> :
      <button key={item.label} role="menuitem" disabled={item.disabled} className={item.danger ? 'menu-danger' : ''}
        onClick={() => { close.current(); item.run() }}>
        {item.icon && <item.icon size={15} />}<span>{item.label}</span>{item.shortcut && <kbd>{item.shortcut}</kbd>}
      </button>)}
  </motion.div>, document.body)
}

export function Kbd({ children }: { children: ReactNode }) { return <kbd className="kbd">{children}</kbd> }
