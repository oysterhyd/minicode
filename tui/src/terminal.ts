import { useEffect, useRef, useState } from 'react'
import { useInput, useStdin, useStdout, type Key } from 'ink'
import { terminalText } from './format.js'

// Claude Code's terminal palette. Light terminals get the darker variants so
// the accent, permission blue and diff backgrounds keep enough contrast.
function lightTerminal(): boolean {
  const forced = (process.env.MINICODE_THEME || '').toLowerCase()
  if (forced === 'light') return true
  if (forced === 'dark') return false
  const background = Number((process.env.COLORFGBG || '').split(';').at(-1))
  return Number.isFinite(background) && (background === 7 || background === 15)
}

const LIGHT = lightTerminal()

export const theme = LIGHT ? {
  claude: '#D77757',
  shimmer: '#F0A585',
  text: '#1F1E1D',
  subtle: '#8A8A8A',
  rule: '#999999',
  success: '#2C7A39',
  error: '#AB2B3F',
  warning: '#966C1E',
  permission: '#5769F7',
  autoAccept: '#8700FF',
  bypass: '#AB2B3F',
  userBg: '#EBEBEB',
  addBg: '#69DB7C',
  delBg: '#FFA8B4',
  addWord: '#2F9D44',
  delWord: '#D1454B',
  code: '#5769F7',
} : {
  claude: '#D77757',
  shimmer: '#EB9F7F',
  text: '#FFFFFF',
  subtle: '#808080',
  rule: '#5A5A5A',
  success: '#4EBA65',
  error: '#FF6B80',
  warning: '#FFC107',
  permission: '#B1B9F9',
  autoAccept: '#AF87FF',
  bypass: '#FF6B80',
  userBg: '#373737',
  addBg: '#225C2B',
  delBg: '#7A2936',
  addWord: '#38A660',
  delWord: '#B3596B',
  code: '#B1B9F9',
}

export const ACCENT = theme.claude

export function enterTerminalScreen(stdout: NodeJS.WriteStream = process.stdout) {
  stdout.write('\x1b[?1049h\x1b[2J\x1b[H\x1b[?2004h\x1b[?1000h\x1b[?1006h')
  let restored = false
  const restore = () => {
    if (restored) return
    restored = true
    process.off('exit', restore)
    stdout.write('\x1b[?1006l\x1b[?1000l\x1b[?2004l\x1b[?1049l\x1b[?25h')
  }
  process.once('exit', restore)
  return restore
}

type TranscriptRenderer = {
  clear: () => void
  fullStaticOutput: string
  lastOutput: string
  lastOutputToRender: string
  lastOutputHeight: number
  log: { sync: (output: string) => void }
}

// Ink's public clear() erases only the live area. Its fullscreen redraw path
// replays fullStaticOutput even after <Static> remounts. Keep this required
// cache reset isolated here and pin Ink's version so the contract is explicit.
const { default: inkInstances } = await import(new URL('./instances.js', import.meta.resolve('ink')).href) as {
  default: WeakMap<NodeJS.WriteStream, TranscriptRenderer>
}

export function clearTranscript(stdout: NodeJS.WriteStream) {
  const renderer = inkInstances.get(stdout)
  if (!renderer) throw new Error('Ink renderer is unavailable')
  renderer.clear()
  stdout.write('\x1b[2J\x1b[H')
  renderer.fullStaticOutput = ''
  renderer.lastOutput = ''
  renderer.lastOutputToRender = ''
  renderer.lastOutputHeight = 0
  renderer.log.sync('')
}

export function useTerminalSize() {
  const { stdout } = useStdout()
  const size = () => ({ columns: stdout.columns || 80, rows: stdout.rows || 24 })
  const [dimensions, setDimensions] = useState(size)
  useEffect(() => {
    const resize = () => setDimensions(size())
    stdout.on('resize', resize)
    return () => { stdout.off('resize', resize) }
  }, [stdout])
  return dimensions
}

export type TerminalMouseEvent = { button: number; x: number; y: number; released: boolean }

export function useTerminalInput(
  handler: (input: string, key: Key, pasted: boolean) => void,
  onMouse?: (event: TerminalMouseEvent) => void,
) {
  const { internal_eventEmitter } = useStdin()
  const raw = useRef('')
  const paste = useRef<string | null>(null)
  useEffect(() => {
    // Ink 6 maps both ASCII DEL (Backspace) and CSI 3~ (Delete) to key.delete.
    // Keep the raw sequence in this one adapter to distinguish the two and to
    // consume bracketed-paste markers before they reach the command editor.
    const capture = (value: string) => { raw.current = value }
    internal_eventEmitter.prependListener('input', capture)
    return () => { internal_eventEmitter.off('input', capture) }
  }, [internal_eventEmitter])
  useInput((input, key) => {
    if (key.eventType === 'release') return
    const sequence = raw.current
    if (sequence === '\x1b[200~') { paste.current = ''; return }
    if (sequence === '\x1b[201~') {
      const value = paste.current
      paste.current = null
      if (value !== null) handler(terminalText(value), key, true)
      return
    }
    if (paste.current !== null) { paste.current += sequence; return }
    const mouse = /^\x1b\[<(\d+);(\d+);(\d+)([Mm])$/.exec(sequence)
    if (mouse) {
      onMouse?.({ button: Number(mouse[1]), x: Number(mouse[2]), y: Number(mouse[3]), released: mouse[4] === 'm' })
      return
    }
    if (sequence === '\x7f' || sequence === '\x1b\x7f') {
      handler('', { ...key, delete: false, backspace: true }, false)
      return
    }
    // A PTY may coalesce printable characters and Enter into one read. Ink
    // treats that whole read as text, so recover control keys outside paste.
    if (sequence.length > 1 && !sequence.includes('\x1b') && /[\x00-\x1f\x7f]/.test(sequence)) {
      for (const chunk of sequence.split(/([\x00-\x1f\x7f])/).filter(Boolean)) {
        if (chunk === '\r') handler('', { ...key, ctrl: false, return: true }, false)
        else if (chunk === '\n') handler('j', { ...key, ctrl: true, return: false }, false)
        else if (chunk === '\t') handler('', { ...key, ctrl: false, tab: true }, false)
        else if (chunk === '\b' || chunk === '\x7f') handler('', { ...key, ctrl: false, backspace: true, delete: false }, false)
        else if (chunk.length === 1 && chunk.charCodeAt(0) > 0 && chunk.charCodeAt(0) <= 26) {
          handler(String.fromCharCode(chunk.charCodeAt(0) + 96), { ...key, ctrl: true }, false)
        } else handler(chunk, { ...key, ctrl: false }, false)
      }
      return
    }
    handler(input, key, false)
  })
}
