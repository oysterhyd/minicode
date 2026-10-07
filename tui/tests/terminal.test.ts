import assert from 'node:assert/strict'
import test from 'node:test'
import { enterTerminalScreen } from '../src/terminal.js'

test('terminal screen starts clear and restores the original buffer and input modes once', () => {
  const writes: string[] = []
  const stdout = { write: (value: string) => { writes.push(value) } } as NodeJS.WriteStream
  const listeners = process.listenerCount('exit')
  const restore = enterTerminalScreen(stdout)
  try {
    assert.equal(writes[0], '\x1b[?1049h\x1b[2J\x1b[H\x1b[?2004h\x1b[?1000h\x1b[?1006h')
    assert.equal(process.listenerCount('exit'), listeners + 1)
    restore()
    restore()
    assert.equal(writes[1], '\x1b[?1006l\x1b[?1000l\x1b[?2004l\x1b[?1049l\x1b[?25h')
    assert.equal(writes.length, 2)
    assert.equal(process.listenerCount('exit'), listeners)
  } finally {
    restore()
  }
})
