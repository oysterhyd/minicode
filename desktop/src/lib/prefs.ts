import { useEffect, useState } from 'react'

export type ThemePreference = 'system' | 'light' | 'dark'
export type Preferences = {
  theme: ThemePreference
  sendKey: 'enter' | 'mod-enter'
  notifications: boolean
  fontScale: number
  leftCollapsed: boolean
  rightCollapsed: boolean
  sidebarWidth: number
  workWidth: number
  expandTools: boolean
}

const defaults: Preferences = {
  theme: 'system', sendKey: 'enter', notifications: true, fontScale: 1,
  leftCollapsed: false, rightCollapsed: false, sidebarWidth: 268, workWidth: 400, expandTools: false,
}

const STORAGE_KEY = 'minicode.preferences.v2'

function load(): Preferences {
  try {
    const saved = JSON.parse(localStorage.getItem(STORAGE_KEY) || 'null')
    if (saved && typeof saved === 'object') return { ...defaults, ...saved }
    // Carry over the preferences stored by the previous desktop layout.
    const theme = localStorage.getItem('minicode.theme')
    return { ...defaults,
      theme: theme === 'light' || theme === 'dark' ? theme : 'system',
      leftCollapsed: localStorage.getItem('minicode.left-collapsed') === 'true',
      rightCollapsed: localStorage.getItem('minicode.right-collapsed') === 'true' }
  } catch { return defaults }
}

export function usePreferences() {
  const [prefs, setPrefs] = useState(load)
  useEffect(() => {
    try { localStorage.setItem(STORAGE_KEY, JSON.stringify(prefs)) } catch { /* Preferences are optional. */ }
  }, [prefs])
  function update<K extends keyof Preferences>(key: K, value: Preferences[K] | ((previous: Preferences[K]) => Preferences[K])) {
    setPrefs(previous => ({ ...previous, [key]: typeof value === 'function' ? (value as (p: Preferences[K]) => Preferences[K])(previous[key]) : value }))
  }
  return [prefs, update] as const
}

export function useSystemDark() {
  const [dark, setDark] = useState(() => window.matchMedia('(prefers-color-scheme: dark)').matches)
  useEffect(() => {
    const media = window.matchMedia('(prefers-color-scheme: dark)')
    const onChange = () => setDark(media.matches)
    media.addEventListener('change', onChange)
    return () => media.removeEventListener('change', onChange)
  }, [])
  return dark
}

const HISTORY_KEY = 'minicode.prompt-history'
export function promptHistory(): string[] {
  try { const value = JSON.parse(localStorage.getItem(HISTORY_KEY) || '[]'); return Array.isArray(value) ? value : [] } catch { return [] }
}
export function rememberPrompt(text: string) {
  try {
    const next = [text, ...promptHistory().filter(item => item !== text)].slice(0, 50)
    localStorage.setItem(HISTORY_KEY, JSON.stringify(next))
  } catch { /* History is a convenience. */ }
}
