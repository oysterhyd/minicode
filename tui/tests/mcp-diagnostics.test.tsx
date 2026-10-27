import assert from 'node:assert/strict'
import test from 'node:test'
import React from 'react'
import { render } from 'ink-testing-library'
import { App } from '../src/App.js'
import { delay, FakeBridge } from './fake-bridge.js'

test('/mcp displays connection status, recent diagnostics and the log path', async () => {
  const client = new FakeBridge()
  client.handlers.set('refreshMcp', () => ({ servers: [{
    server: 'fixture', error: 'MCP connection failed\nMCP stderr:\nmissing config',
    stderr_tail: 'missing config', log_path: 'C:\\logs\\mcp.log',
  }], tools: [] }))
  const { stdin, lastFrame, unmount } = render(
    React.createElement(App, { client, options: { workspace: 'D:\\demo', provider: 'fake' }, version: '1.0.0' }),
  )
  try {
    await delay(100)
    stdin.write('/mcp')
    await delay(40)
    stdin.write('\r')
    await delay(120)
    assert.equal(client.requests.filter(request => request.method === 'refreshMcp').length, 1, JSON.stringify({ requests: client.requests, frame: lastFrame() }))
    assert.match(lastFrame() || '', /fixture/)
    assert.match(lastFrame() || '', /missing config/)
    assert.match(lastFrame() || '', /mcp\.log/)
  } finally {
    unmount()
  }
})
