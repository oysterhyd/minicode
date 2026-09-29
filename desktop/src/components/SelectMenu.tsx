import { useEffect, useRef, type ReactNode } from 'react'
import { AnimatePresence, motion } from 'motion/react'
import { Check, ChevronDown } from 'lucide-react'

type Option = { value: string; label: string; detail?: string; disabled?: boolean }

export function SelectMenu({ label, icon, value, options, secondary, open, onOpen, onSelect }: {
  label: string; icon?: ReactNode; value: string; open: boolean; onOpen: (open: boolean) => void;
  onSelect: (value: string) => void; options: Option[];
  secondary?: { label: string; value: string; options: Option[]; onSelect: (value: string) => void; hint?: string }
}) {
  const root = useRef<HTMLDivElement>(null)
  const trigger = useRef<HTMLButtonElement>(null)
  useEffect(() => {
    if (!open) return
    const close = (event: PointerEvent) => { if (!root.current?.contains(event.target as Node)) onOpen(false) }
    document.addEventListener('pointerdown', close)
    const selected = root.current?.querySelector<HTMLElement>('[aria-checked="true"]:not(:disabled)')
    ;(selected || root.current?.querySelector<HTMLElement>('[role="menuitemradio"]:not(:disabled)'))?.focus()
    return () => document.removeEventListener('pointerdown', close)
  }, [open])
  return <div className="select-menu" ref={root} onKeyDown={event => {
    if (event.key === 'Escape') { event.stopPropagation(); onOpen(false); trigger.current?.focus() }
    if (event.key === 'Tab') onOpen(false)
    if (!open || !['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return
    event.preventDefault()
    const buttons = Array.from(root.current!.querySelectorAll<HTMLButtonElement>('[role="menuitemradio"]:not(:disabled)'))
    const index = buttons.indexOf(document.activeElement as HTMLButtonElement)
    buttons[event.key === 'Home' ? 0 : event.key === 'End' ? buttons.length - 1 : (index + (event.key === 'ArrowDown' ? 1 : -1) + buttons.length) % buttons.length]?.focus()
  }}>
    <button ref={trigger} className="composer-control" aria-label={label} aria-haspopup="menu" aria-expanded={open} onClick={() => onOpen(!open)}>{icon}<span className="truncate">{options.find(option => option.value === value)?.label || value}</span><ChevronDown size={12} className={open ? 'rotate-180' : ''} /></button>
    <AnimatePresence>{open && <motion.div role="menu" aria-label={label} className={`composer-menu ${secondary ? 'composer-menu-with-secondary' : ''}`} initial={{ opacity: 0, y: 6 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: 3 }} transition={{ duration: .14 }}>
      <div className="section-label">{label}</div>{options.map(option => <button key={option.value} role="menuitemradio" aria-checked={value === option.value} aria-label={`${option.label}${option.detail ? ` · ${option.detail}` : ''}`} title={option.detail} disabled={option.disabled}
        onClick={() => { onSelect(option.value); onOpen(false); trigger.current?.focus() }}>
        <span><span>{option.label}</span>{option.detail && <small>{option.detail}</small>}</span><Check size={14} style={{ opacity: value === option.value ? 1 : 0 }} />
      </button>)}
      {secondary && <><div className="composer-menu-divider" /><div className="section-label composer-menu-subheading">{secondary.label}{secondary.hint && <small>{secondary.hint}</small>}</div>
        <div className="composer-menu-secondary-options">{secondary.options.map(option => <button key={`secondary-${option.value}`} role="menuitemradio" aria-label={`${secondary.label} ${option.label}`} aria-checked={secondary.value === option.value} disabled={option.disabled}
          onClick={() => { secondary.onSelect(option.value); onOpen(false); trigger.current?.focus() }}>
          <span><span>{option.label}</span>{option.detail && <small>{option.detail}</small>}</span><Check size={14} style={{ opacity: secondary.value === option.value ? 1 : 0 }} />
        </button>)}</div></>}
    </motion.div>}</AnimatePresence>
  </div>
}
