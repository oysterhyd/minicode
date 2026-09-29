import { useEffect, useRef, type ReactNode } from 'react'
import { motion } from 'motion/react'

const FOCUSABLE = 'button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), a[href], [tabindex="0"]'

export function Modal({ children, label, className = '', onClose }: {
  children: ReactNode; label: string; className?: string; onClose: () => void
}) {
  const dialog = useRef<HTMLElement>(null)
  const close = useRef(onClose)
  close.current = onClose
  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null
    const root = dialog.current!
    const focusable = () => Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(element => element.getClientRects().length > 0)
    ;(root.querySelector<HTMLElement>('[data-autofocus]') || focusable()[0] || root).focus()
    const keydown = (event: KeyboardEvent) => {
      // Only the top-most dialog handles keys.
      const dialogs = document.querySelectorAll('[role="dialog"], [role="alertdialog"]')
      if (dialogs[dialogs.length - 1] !== root) return
      if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); close.current(); return }
      if (event.key !== 'Tab') return
      const elements = focusable()
      const first = elements[0], last = elements.at(-1)
      if (!first) { event.preventDefault(); root.focus(); return }
      if (event.shiftKey && (document.activeElement === first || !root.contains(document.activeElement))) {
        event.preventDefault(); last?.focus()
      } else if (!event.shiftKey && (document.activeElement === last || !root.contains(document.activeElement))) {
        event.preventDefault(); first.focus()
      }
    }
    document.addEventListener('keydown', keydown, true)
    return () => {
      document.removeEventListener('keydown', keydown, true)
      const anotherDialog = Array.from(document.querySelectorAll('[role="dialog"], [role="alertdialog"]')).some(element => element !== root)
      if (!anotherDialog && previous?.isConnected) previous.focus()
    }
  }, [])
  return <motion.div className="modal-backdrop" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} transition={{ duration: .18 }}
    onMouseDown={event => { if (event.target === event.currentTarget) onClose() }}>
    <motion.section ref={dialog} tabIndex={-1} className={`modal-dialog ${className}`} role="dialog" aria-modal="true" aria-label={label}
      initial={{ opacity: 0, y: 14, scale: .97 }} animate={{ opacity: 1, y: 0, scale: 1 }} exit={{ opacity: 0, y: 8, scale: .98, transition: { duration: .14 } }}
      transition={{ type: 'spring', stiffness: 480, damping: 36, mass: .8 }}>
      {children}
    </motion.section>
  </motion.div>
}

export function ConfirmDialog({ title, detail, confirmLabel, danger, onConfirm, onClose }: {
  title: string; detail: ReactNode; confirmLabel: string; danger?: boolean; onConfirm: () => void; onClose: () => void
}) {
  return <Modal label={title} className="confirm-dialog" onClose={onClose}>
    <h2>{title}</h2>
    <div className="confirm-detail">{detail}</div>
    <footer>
      <button className="button button-ghost" onClick={onClose}>取消</button>
      <button data-autofocus className={`button ${danger ? 'button-danger' : 'button-primary'}`} onClick={() => { onConfirm(); onClose() }}>{confirmLabel}</button>
    </footer>
  </Modal>
}
