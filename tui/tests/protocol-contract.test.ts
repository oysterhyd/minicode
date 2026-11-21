import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import test from 'node:test'
import { fileURLToPath } from 'node:url'
import {
  acceptsRunEvent,
  applyDesktopEvent,
  emptyFeed,
  formatToolResultOutput,
  noticeForAgentEvent,
  type FeedState,
} from '../src/feed.js'
import type { DesktopEvent } from '../src/types.js'

type ProtocolManifest = {
  protocolVersion: number
  minClientProtocol: number
  events: {
    critical: string[]
    transient: string[]
  }
  stateFields: string[]
  conformance: {
    runGateCases: Array<{
      name: string
      view: {
        sessionId: string
        abandoned: string[]
        runId: string
        awaitingRun: boolean
        busy: boolean
      }
      event: {
        event: DesktopEvent['event']
        sessionId: string
        runId: string
      }
      expected: boolean
    }>
    noticeCases: Array<{
      name: string
      type: string
      data: Record<string, unknown>
      expectedText: string
      expectedTone: 'info' | 'warning' | ''
    }>
    toolOutputCases: Array<{
      name: string
      data: Record<string, unknown>
      expected: string
    }>
  }
}

function findRepoRoot(): string {
  let current = path.dirname(fileURLToPath(import.meta.url))
  for (let i = 0; i < 8; i++) {
    const candidate = path.join(current, 'agent', 'assets', 'bridge-protocol.v2.json')
    if (fs.existsSync(candidate)) {
      return current
    }
    const parent = path.dirname(current)
    if (parent === current) break
    current = parent
  }
  throw new Error('Could not locate agent/assets/bridge-protocol.v2.json from test file location')
}

function loadManifest(): { repoRoot: string; manifest: ProtocolManifest } {
  const repoRoot = findRepoRoot()
  const raw = fs.readFileSync(path.join(repoRoot, 'agent', 'assets', 'bridge-protocol.v2.json'), 'utf8')
  return { repoRoot, manifest: JSON.parse(raw) as ProtocolManifest }
}

test('protocol manifest matches TUI types, event coverage, and shared conformance vectors', () => {
  const { repoRoot, manifest } = loadManifest()
  assert.equal(manifest.protocolVersion, 2)
  assert.equal(manifest.minClientProtocol, 2)

  const typesSource = fs.readFileSync(path.join(repoRoot, 'tui', 'src', 'types.ts'), 'utf8')
  const agentStateMatch = typesSource.match(/export type AgentState = \{([\s\S]*?)\r?\n\}/)
  assert.ok(agentStateMatch, 'AgentState type block should exist in tui/src/types.ts')
  for (const field of manifest.stateFields) {
    assert.match(
      agentStateMatch[1],
      new RegExp(`^\\s*${field}\\??:`, 'm'),
      `AgentState in tui/src/types.ts is missing protocol field "${field}"`,
    )
  }

  for (const eventName of [...manifest.events.critical, ...manifest.events.transient]) {
    const base = emptyFeed()
    const event: DesktopEvent = {
      event: eventName as DesktopEvent['event'],
      sessionId: 's1',
      runId: eventName === 'run_started' ? 'r1' : '',
      text: eventName === 'warning' || eventName === 'text_delta' || eventName === 'reasoning_delta' ? 'sample' : undefined,
      usage: eventName === 'usage_delta' ? { input_tokens: 10, output_tokens: 5 } : undefined,
      result: eventName === 'run_done' ? { exit_reason: 'completed' } : undefined,
      error: eventName === 'run_error' || eventName === 'bridge_error' ? 'sample error' : undefined,
    }
    const next = applyDesktopEvent(base, event)
    assert.ok(next && Array.isArray(next.items), `applyDesktopEvent should handle "${eventName}" cleanly`)
    if (eventName === 'log') {
      assert.equal(next, base)
    }
    if (eventName === 'usage_delta') {
      assert.deepEqual(next.liveUsage, { input_tokens: 10, output_tokens: 5 })
    }
  }

  for (const tc of manifest.conformance.runGateCases) {
    const state: FeedState = {
      ...emptyFeed(),
      sessionId: tc.view.sessionId || null,
      abandonedSessions: new Set(tc.view.abandoned),
      runId: tc.view.runId || null,
      awaitingRun: tc.view.awaitingRun,
      busy: tc.view.busy,
    }
    assert.equal(
      acceptsRunEvent(state, tc.event as DesktopEvent),
      tc.expected,
      `runGateCase "${tc.name}" failed`,
    )
  }

  for (const tc of manifest.conformance.noticeCases) {
    const got = noticeForAgentEvent(tc.type, tc.data)
    if (!tc.expectedText) {
      assert.equal(got, null, `noticeCase "${tc.name}" expected null`)
    } else {
      assert.deepEqual(
        got,
        { text: tc.expectedText, tone: tc.expectedTone },
        `noticeCase "${tc.name}" failed`,
      )
    }
  }

  for (const tc of manifest.conformance.toolOutputCases) {
    assert.equal(
      formatToolResultOutput(tc.data),
      tc.expected,
      `toolOutputCase "${tc.name}" failed`,
    )
  }
})
