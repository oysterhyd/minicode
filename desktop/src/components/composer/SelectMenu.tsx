import { useEffect, useRef, type ReactNode } from 'react'
import { AnimatePresence, motion } from 'motion/react'
import { Check, ChevronDown } from 'lucide-react'

type Option = { value: string; label: string; detail?: string; disabled?: boolean }

export function SelectMenu({ label, icon, value, options, secondary, open, onOpen, onSelect, tip }: {
  label: string; icon?: ReactNode; value: string; open: boolean; onOpen: (open: boolean) => void; tip?: string
  onSelect: (value: string) => void; options: Option[]
  secondary?: { label: string; value: string; options: Option[]; onSelect: (value: string) => void; hint?: string }
}) {
  const root = useRef<HTMLDivElement>(null)
  const trigger = useRef<HTMLButtonElement>(null)
  useEffect(() => {
    if (!open) return
    const close = (event: PointerEvent) => { if (!root.current?.contains(event.target as Node)) onOpen(false) }
    document.addEventListener('pointerdown', close)
    const selected = root.current?.querySelector<HTMLElement>('[role="menuitemradio"][aria-checked="true"]:not(:disabled)')
    ;(selected || root.current?.querySelector<HTMLElement>('[role="menuitemradio"]:not(:disabled)'))?.focus()
    return () => document.removeEventListener('pointerdown', close)
  }, [open])
  const current = options.find(option => option.value === value)
  return <div className="select-menu" ref={root} onKeyDown={event => {
    if (event.key === 'Escape' && open) { event.stopPropagation(); onOpen(false); trigger.current?.focus() }
    if (event.key === 'Tab') onOpen(false)
    if (!open || !['ArrowDown', 'ArrowUp', 'ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return
    event.preventDefault()
    const buttons = Array.from(root.current!.querySelectorAll<HTMLButtonElement>('[role="menuitemradio"]:not(:disabled)'))
    const index = buttons.indexOf(document.activeElement as HTMLButtonElement)
    const forward = event.key === 'ArrowDown' || event.key === 'ArrowRight'
    buttons[event.key === 'Home' ? 0 : event.key === 'End' ? buttons.length - 1 : (index + (forward ? 1 : -1) + buttons.length) % buttons.length]?.focus()
  }}>
    <button ref={trigger} className={`composer-chip ${open ? 'is-open' : ''}`} aria-label={label} aria-haspopup="menu" aria-expanded={open} data-tip={open ? undefined : tip || label} onClick={() => onOpen(!open)}>
      {icon}<span className="truncate">{current?.label || value}</span>
      {secondary && secondary.value !== 'off' && <span className="chip-sub">{secondary.options.find(option => option.value === secondary.value)?.label}</span>}
      <ChevronDown size={12} className={`chip-chevron ${open ? 'is-open' : ''}`} />
    </button>
    <AnimatePresence>{open && <motion.div role="menu" aria-label={label} className="composer-menu"
      initial={{ opacity: 0, y: 6, scale: .98 }} animate={{ opacity: 1, y: 0, scale: 1 }} exit={{ opacity: 0, y: 4, scale: .98 }} transition={{ duration: .14, ease: [.2, .8, .2, 1] }}>
      <div className="menu-heading">{label}</div>
      {options.map(option => <button key={option.value} role="menuitemradio" aria-checked={value === option.value} aria-label={`${option.label}${option.detail ? ` · ${option.detail}` : ''}`} disabled={option.disabled}
        className="menu-option" onClick={() => { onSelect(option.value); onOpen(false); trigger.current?.focus() }}>
        <span className="menu-option-text"><span>{option.label}</span>{option.detail && <small>{option.detail}</small>}</span>
        <Check size={14} className="menu-check" style={{ opacity: value === option.value ? 1 : 0 }} />
      </button>)}
      {secondary && <>
        <div className="menu-divider" />
        <div className="menu-heading">{secondary.label}{secondary.hint && <small>{secondary.hint}</small>}</div>
        <div className="segmented" role="group" aria-label={secondary.label}>{secondary.options.map(option => <button key={option.value} role="menuitemradio"
          aria-label={`${secondary.label} ${option.label}`} aria-checked={secondary.value === option.value} disabled={option.disabled}
          className={`segment ${secondary.value === option.value ? 'is-active' : ''}`} onClick={() => { secondary.onSelect(option.value); onOpen(false); trigger.current?.focus() }}>
          {secondary.value === option.value && <motion.span layoutId={`${label}-segment`} className="segment-indicator" transition={{ type: 'spring', stiffness: 500, damping: 38 }} />}
          <span>{option.label}</span>
        </button>)}</div>
      </>}
    </motion.div>}</AnimatePresence>
  </div>
}
