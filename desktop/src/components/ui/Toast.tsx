import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from 'react'
import { AnimatePresence, motion } from 'motion/react'
import { CircleAlert, CircleCheck, Info, X } from 'lucide-react'

type Tone = 'success' | 'error' | 'info'
type Toast = { id: number; tone: Tone; title: string; detail?: string; action?: { label: string; run: () => void } }
type Push = (toast: Omit<Toast, 'id'>) => void

const ToastContext = createContext<Push>(() => {})
export const useToast = () => useContext(ToastContext)

const icons = { success: CircleCheck, error: CircleAlert, info: Info }

function ToastItem({ toast, onClose }: { toast: Toast; onClose: () => void }) {
  const [hover, setHover] = useState(false)
  const close = useRef(onClose)
  close.current = onClose
  useEffect(() => {
    if (hover) return
    const timer = setTimeout(() => close.current(), toast.tone === 'error' ? 7000 : 4200)
    return () => clearTimeout(timer)
  }, [hover, toast.tone])
  const Icon = icons[toast.tone]
  return <motion.div layout className={`toast toast-${toast.tone}`} role={toast.tone === 'error' ? 'alert' : 'status'}
    initial={{ opacity: 0, y: 16, scale: .96 }} animate={{ opacity: 1, y: 0, scale: 1 }} exit={{ opacity: 0, x: 24, scale: .96, transition: { duration: .16 } }}
    transition={{ type: 'spring', stiffness: 520, damping: 38 }} onPointerEnter={() => setHover(true)} onPointerLeave={() => setHover(false)}>
    <Icon size={16} className="toast-icon" />
    <div className="toast-body"><strong>{toast.title}</strong>{toast.detail && <span>{toast.detail}</span>}</div>
    {toast.action && <button className="toast-action" onClick={() => { toast.action!.run(); onClose() }}>{toast.action.label}</button>}
    <button className="toast-close" aria-label="关闭通知" onClick={onClose}><X size={13} /></button>
  </motion.div>
}

export function ToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<Toast[]>([])
  const next = useRef(1)
  const push = useCallback<Push>(toast => {
    setToasts(previous => [...previous.filter(item => item.title !== toast.title || item.detail !== toast.detail), { ...toast, id: next.current++ }].slice(-4))
  }, [])
  return <ToastContext.Provider value={push}>
    {children}
    <div className="toast-region" aria-live="polite">
      <AnimatePresence initial={false}>{toasts.map(toast => <ToastItem key={toast.id} toast={toast}
        onClose={() => setToasts(previous => previous.filter(item => item.id !== toast.id))} />)}</AnimatePresence>
    </div>
  </ToastContext.Provider>
}
