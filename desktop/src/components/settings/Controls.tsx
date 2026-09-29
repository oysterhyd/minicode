import type { ReactNode } from 'react'
import { motion } from 'motion/react'

export function Section({ title, description, children, action }: { title: string; description?: ReactNode; children?: ReactNode; action?: ReactNode }) {
  return <section className="settings-section">
    <header><div><h3>{title}</h3>{description && <p>{description}</p>}</div>{action}</header>
    {children && <div className="settings-section-body">{children}</div>}
  </section>
}

export function Row({ label, description, children }: { label: string; description?: ReactNode; children: ReactNode }) {
  return <div className="settings-row"><div className="settings-row-text"><strong>{label}</strong>{description && <span>{description}</span>}</div><div className="settings-row-control">{children}</div></div>
}

export function Toggle({ checked, onChange, label }: { checked: boolean; onChange: (value: boolean) => void; label: string }) {
  return <button role="switch" aria-checked={checked} aria-label={label} className={`toggle ${checked ? 'is-on' : ''}`} onClick={() => onChange(!checked)}>
    <motion.span className="toggle-thumb" layout transition={{ type: 'spring', stiffness: 700, damping: 40 }} />
  </button>
}

export function Segmented<T extends string>({ value, options, onChange, label }: { value: T; options: Array<[T, string]>; onChange: (value: T) => void; label: string }) {
  return <div className="segmented" role="radiogroup" aria-label={label}>
    {options.map(([option, text]) => <button key={option} role="radio" aria-checked={value === option} className={`segment ${value === option ? 'is-active' : ''}`} onClick={() => onChange(option)}>
      {value === option && <motion.span layoutId={`settings-${label}`} className="segment-indicator" transition={{ type: 'spring', stiffness: 500, damping: 38 }} />}
      <span>{text}</span>
    </button>)}
  </div>
}

export function Empty({ children }: { children: ReactNode }) { return <p className="settings-empty">{children}</p> }
