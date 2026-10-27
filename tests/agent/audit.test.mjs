import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import http from 'node:http'
import { spawnSync } from 'node:child_process'
import { test } from 'node:test'
import { fixture, connect, executable, projectRoot } from './helpers.mjs'

async function modelServer(t, f, reply, style = 'openai') {
  const server = http.createServer((req, res) => {
    let data = ''
    req.on('data', chunk => { data += chunk })
    req.on('end', () => reply(JSON.parse(data), res))
  })
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
  t.after(() => { server.closeAllConnections(); return new Promise(resolve => server.close(resolve)) })
  const config = await f.request('saveService', { service: {
    name: 'Independent audit server', baseUrl: `http://127.0.0.1:${server.address().port}/v1`,
    apiStyle: style, apiKey: 'audit-fixture-key', enabled: true,
    models: [{ modelId: 'audit-model', contextWindow: 200000, maxOutputTokens: 8192, supportsEffort: false, capabilities: {} }],
  } })
  return config.defaultModel
}

function openai(res, content, calls = []) {
  res.setHeader('content-type', 'application/json')
  res.end(JSON.stringify({ choices: [{ message: { content, tool_calls: calls }, finish_reason: calls.length ? 'tool_calls' : 'stop' }], usage: { prompt_tokens: 10, completion_tokens: 5 } }))
}
function call(id, name, args) { return { id, type: 'function', function: { name, arguments: JSON.stringify(args) } } }

test('truncated malformed tool JSON is continued without executing the incomplete write', async t => {
  const f = await fixture(t)
  let requests = 0
  const model = await modelServer(t, f, (_body, res) => {
    if (requests++ === 0) {
      res.setHeader('content-type', 'text/event-stream')
      res.end('data: ' + JSON.stringify({ choices: [{ delta: { tool_calls: [{ index: 0, id: 'incomplete', function: { name: 'write', arguments: '{"path":"unsafe.txt",' } }] }, finish_reason: 'length' }], usage: { prompt_tokens: 10, completion_tokens: 5 } }) + '\n\ndata: [DONE]\n\n')
    } else openai(res, 'Recovered with a complete answer')
  })
  await f.request('setPermissionMode', { mode: 'bypass' })
  const r = await f.run({ model })
  assert.equal(r.result.exit_reason, 'completed')
  assert.equal(requests, 2)
  assert.equal(fs.existsSync(path.join(f.workspace, 'unsafe.txt')), false)
  assert.equal(r.result.total_usage.output_tokens, 10)
})

test('model SSE accepts BOM, multiline events and CR-only framing across UTF-8 chunks', async t => {
  const f = await fixture(t)
  const model = await modelServer(t, f, (_body, res) => {
    res.setHeader('content-type', 'text/event-stream')
    const payload = Buffer.from('\ufeff: heartbeat\rdata: {"choices":[{"delta":{"content":"中文😀"},\rdata: "finish_reason":"stop"}],"usage":{"prompt_tokens":11,"completion_tokens":4}}\r\rdata: [DONE]\r\r')
    for (let i = 0; i < payload.length; i += 2) res.write(payload.subarray(i, i + 2))
    res.end()
  })
  const r = await f.run({ model })
  assert.equal(r.result.exit_reason, 'completed')
  assert.equal(r.detail.messages.at(-1).content[0].text, '中文😀')
  assert.equal(r.result.total_usage.input_tokens, 11)
})

test('delegates inherit loaded skills but never execute the parent acceptance command', async t => {
  const f = await fixture(t)
  const skill = path.join(f.workspace, '.minicode/skills/audit')
  fs.mkdirSync(skill, { recursive: true })
  fs.writeFileSync(path.join(skill, 'SKILL.md'), '---\nname: audit\ndescription: audit fixture\n---\nPINNED_SKILL_MARKER\n')
  const spec = path.join(f.root, 'acceptance.json')
  const command = process.platform === 'win32' ? "Add-Content -Path 'checks.txt' -Value 'check'" : "printf 'check\\n' >> checks.txt"
  fs.writeFileSync(spec, JSON.stringify({ items: [{ id: 'check', type: 'command', command }] }))
  await f.request('setAcceptance', { path: spec })
  await f.request('setSkillActive', { workspace: f.workspace, name: 'audit', active: true })
  let parent = 0, children = 0
  const model = await modelServer(t, f, (body, res) => {
    const system = body.messages.find(m => m.role === 'system')?.content || ''
    if (system.includes('你是子助手。')) {
      children++
      assert.ok(system.includes('PINNED_SKILL_MARKER'))
      openai(res, JSON.stringify({ summary: 'Checked', findings: [], evidence_refs: [], unresolved: [] }))
    } else if (parent++ === 0) openai(res, '', [call('delegate', 'delegate', { kind: 'explore', task: 'Inspect safely' })])
    else openai(res, 'Done')
  })
  const r = await f.run({ model })
  assert.equal(r.result.exit_reason, 'completed')
  assert.equal(children, 1)
  assert.equal(fs.readFileSync(path.join(f.workspace, 'checks.txt'), 'utf8').trim().split(/\r?\n/).length, 1)
})

test('skill_load persists activation through a bridge restart', async t => {
  const f = await fixture(t, [{ tool_calls: [{ name: 'skill_load', arguments: { name: 'audit' } }] }, { text: 'Done' }])
  const dir = path.join(f.workspace, '.minicode/skills/audit')
  fs.mkdirSync(dir, { recursive: true })
  fs.writeFileSync(path.join(dir, 'SKILL.md'), '---\nname: audit\ndescription: fixture\n---\nKeep these rules\n')
  await f.request('setBudget', { max_rounds: 1 })
  const r = await f.run()
  assert.equal(r.result.exit_reason, 'max_rounds')
  await f.close()
  const c = connect(f)
  await c.request('selectSession', { sessionId: r.id })
  const caps = await c.request('getCapabilities', { sessionId: r.id, workspace: f.workspace })
  assert.equal(caps.skills.find(s => s.name === 'audit').active, true)
  const lazy = connect(f)
  await lazy.request('selectSession', { sessionId: r.id, deferRuntime: true })
  const lazyCaps = await lazy.request('getCapabilities', { sessionId: r.id, workspace: f.workspace })
  assert.equal(lazyCaps.skills.find(s => s.name === 'audit').active, true)
})

test('a shell cwd discovers nested instructions before any side effects', async t => {
  const command = process.platform === 'win32' ? "Set-Content touched.txt 'unsafe'" : "printf unsafe > touched.txt"
  const f = await fixture(t, [{ tool_calls: [{ name: 'bash', arguments: { command, cwd: 'sub' } }] }, { text: 'Read the newly loaded rules' }])
  fs.mkdirSync(path.join(f.workspace, 'sub'))
  fs.writeFileSync(path.join(f.workspace, 'sub/AGENTS.md'), 'Do not create touched.txt')
  await f.request('setPermissionMode', { mode: 'bypass' })
  const r = await f.run()
  assert.equal(fs.existsSync(path.join(f.workspace, 'sub/touched.txt')), false)
  assert.ok(r.detail.events.some(e => e.type === 'project_instructions'))
})

test('reselecting the same fake model does not restart its script', async t => {
  const f = await fixture(t, [{ text: 'first' }, { text: 'second' }])
  const first = await f.run()
  await f.request('setModel', { sessionId: first.id, model: 'fake' })
  const second = await f.run({ sessionId: first.id })
  assert.equal(second.detail.messages.at(-1).content[0].text, 'second')
})

test('an exact token cap suppresses writes and effort cannot bypass model capabilities', async t => {
  const f = await fixture(t, [{ input_tokens: 80, output_tokens: 20, tool_calls: [{ name: 'write', arguments: { path: 'over-cap.txt', content: 'unsafe' } }] }])
  await f.request('setBudget', { max_total_tokens: 100 })
  await f.request('setPermissionMode', { mode: 'bypass' })
  const r = await f.run()
  assert.equal(r.result.exit_reason, 'token_budget')
  assert.equal(fs.existsSync(path.join(f.workspace, 'over-cap.txt')), false)
  const model = await modelServer(t, f, (_body, res) => openai(res, 'done'))
  await f.request('setModel', { model })
  await assert.rejects(f.request('setEffort', { effort: 'high' }), /不支持/)
})

test('stale in-memory sessions reload the latest history before accepting another prompt', async t => {
  const f = await fixture(t, [{ text: 'initial' }, { text: 'external update' }])
  const first = await f.run()
  const c = connect(f)
  await c.request('selectSession', { sessionId: first.id })
  await f.run({ sessionId: first.id, text: 'external prompt' })
  const from = c.events.length
  await c.request('sendPrompt', { sessionId: first.id, workspace: f.workspace, model: 'fake', text: 'stale prompt' })
  const done = await c.wait(e => e.event === 'run_done', from)
  assert.equal(done.result.exit_reason, 'completed')
  const detail = await f.request('getSession', { sessionId: first.id })
  assert.ok(JSON.stringify(detail.messages).includes('external prompt'))
  assert.ok(JSON.stringify(detail.messages).includes('external update'))
  assert.ok(JSON.stringify(detail.messages).includes('stale prompt'))
})

function cliEnvironment(f) {
  return { ...process.env, HOME: f.root, USERPROFILE: f.root, MINICODE_DB_PATH: f.db, MINICODE_FAKE_SCRIPT: '', MINICODE_RESOURCES: '', ANTHROPIC_API_KEY: '', COMMANDCODE_API_KEY: '' }
}

test('CLI command errors do not exit chat and relative database paths work', async t => {
  const f = await fixture(t, [{ text: 'CLI_RECOVERED' }])
  const result = spawnSync(executable, ['chat', '--provider', 'fake', '--script', f.script, '--workspace', f.workspace, '--db', 'relative.db'], {
    cwd: f.workspace, env: cliEnvironment(f), input: '/permissions invalid\nhello\n/quit\n', encoding: 'utf8', timeout: 8000,
  })
  assert.equal(result.status, 0, result.stderr || String(result.error))
  assert.match(result.stderr, /无效权限模式/)
  assert.match(result.stdout, /CLI_RECOVERED/)
  assert.equal(fs.existsSync(path.join(f.workspace, 'relative.db')), true)
})

test('CLI --script resume continues from the saved response instead of replaying writes', async t => {
  const f = await fixture(t, [{ tool_calls: [{ name: 'write', arguments: { path: 'once.txt', content: 'once' } }] }, { text: 'RESUME_MARKER' }])
  const options = { cwd: f.workspace, env: cliEnvironment(f), encoding: 'utf8', timeout: 8000 }
  const first = spawnSync(executable, ['run', 'task', '--script', f.script, '--workspace', f.workspace, '--max-rounds', '1', '--yes'], options)
  assert.equal(first.status, 2, first.stderr)
  const [session] = await f.request('listSessions')
  const second = spawnSync(executable, ['resume', session.session_id, '--script', f.script, '--workspace', f.workspace, '--yes'], { ...options, input: '/continue\n/quit\n' })
  assert.equal(second.status, 0, second.stderr)
  assert.match(second.stdout, /RESUME_MARKER/)
  const detail = await f.request('getSession', { sessionId: session.session_id })
  assert.equal(detail.events.filter(e => e.type === 'tool_call_result' && e.data.name === 'write').length, 1)
})

test('global CLI --db is forwarded as an absolute path to the TUI launcher', async t => {
  const f = await fixture(t)
  const dir = path.join(f.root, 'runtime/tui/dist')
  fs.mkdirSync(dir, { recursive: true })
  fs.writeFileSync(path.join(dir, 'index.js'), 'console.log("DB="+process.env.MINICODE_DB_PATH)')
  const result = spawnSync(executable, ['--db', 'selected.db', 'tui'], {
    cwd: f.workspace, env: { ...cliEnvironment(f), MINICODE_RESOURCES: f.root }, encoding: 'utf8', timeout: 8000,
  })
  assert.equal(result.status, 0, result.stderr)
  assert.match(result.stdout, /DB=/)
  const actual = result.stdout.trim().slice(3)
  assert.equal(path.isAbsolute(actual), true)
  // Windows runners may expose TEMP through an 8.3 alias. Compare the existing
  // directories by identity; selected.db does not exist until the real TUI opens it.
  assert.equal(fs.realpathSync.native(path.dirname(actual)), fs.realpathSync.native(f.workspace))
  assert.equal(path.basename(actual), 'selected.db')
})

test('archived tool ids remain reserved after manual compaction', async t => {
  const f = await fixture(t, [
    { text: 'initial reply' },
    { tool_calls: [{ id: 'archived-call', name: 'read', arguments: { path: 'README.md' } }] },
    { text: 'Middle task findings\n'.repeat(1200) },
    { text: 'latest reply' },
    { tool_calls: [{ id: 'archived-call', name: 'read', arguments: { path: 'README.md' } }] },
  ])
  await f.request('setPermissionMode', { mode: 'bypass' })
  const first = await f.run({ text: 'KEEP_ORIGINAL_REQUIREMENT' })
  await f.run({ sessionId: first.id, text: 'middle task' })
  await f.run({ sessionId: first.id, text: 'latest task' })
  await f.request('runSlash', { text: '/compact', workspace: f.workspace, sessionId: first.id })
  const detail = await f.request('getSession', { sessionId: first.id })
  assert.equal(detail.messages[0].content[0].text, 'KEEP_ORIGINAL_REQUIREMENT')
  assert.equal(detail.messages.flatMap(m => m.content).some(b => b.type === 'tool_use' && b.id === 'archived-call'), false)
  const reused = await f.run({ sessionId: first.id })
  assert.equal(reused.result.exit_reason, 'completed')
  const call = reused.detail.messages.flatMap(m => m.content).find(b => b.type === 'tool_use')
  assert.ok(call.id && call.id !== 'archived-call')
  assert.ok(reused.detail.messages.flatMap(m => m.content).some(b => b.type === 'tool_result' && b.tool_use_id === call.id))
})

test('project skills override user skills with the same name', async t => {
  const f = await fixture(t, [{ tool_calls: [{ name: 'skill_load', arguments: { name: 'same' } }] }, { text: 'done' }])
  for (const [root, marker] of [[f.root, 'GLOBAL_BODY'], [f.workspace, 'PROJECT_BODY']]) {
    const dir = path.join(root, '.minicode/skills/same')
    fs.mkdirSync(dir, { recursive: true })
    fs.writeFileSync(path.join(dir, 'SKILL.md'), '---\nname: same\ndescription: same name\n---\n' + marker)
  }
  const r = await f.run()
  assert.equal(r.result.exit_reason, 'completed')
  const result = r.detail.events.find(e => e.type === 'tool_call_result')
  assert.match(result.data.output_detail, /PROJECT_BODY/)
  assert.doesNotMatch(result.data.output_detail, /GLOBAL_BODY/)
})

test('CLI output filters terminal controls and text reports honor --output', async t => {
  const f = await fixture(t, [{ tool_calls: [{ name: 'read', arguments: { path: 'control.txt' } }, { name: 'read', arguments: { path: 'missing.txt' } }] }, { text: 'MODEL\x1b[2J\x07DONE' }])
  fs.writeFileSync(path.join(f.workspace, 'control.txt'), 'FILE\x1b]52;c;Y2xpcGJvYXJk\x07BODY\n')
  const options = { cwd: f.workspace, env: cliEnvironment(f), encoding: 'utf8', timeout: 8000 }
  const result = spawnSync(executable, ['run', 'inspect', '--script', f.script, '--workspace', f.workspace], options)
  assert.equal(result.status, 0, result.stderr)
  assert.ok(!result.stdout.includes('\x1b') && !result.stdout.includes('\x07'))
  assert.match(result.stdout, /MODEL/)
  assert.match(result.stdout, /path is not a file/)
  const [session] = await f.request('listSessions')
  const output = path.join(f.root, '审查 report.txt')
  const report = spawnSync(executable, ['report', session.session_id, '--format', 'text', '--output', output], options)
  assert.equal(report.status, 0, report.stderr)
  const text = fs.readFileSync(output, 'utf8')
  assert.match(text, /MODEL/)
  assert.match(text, /BODY/)
  assert.ok(!text.includes('\x1b') && !text.includes('\x07'))
})

test('a deleted empty session cannot run tools or create orphan history', async t => {
  const f = await fixture(t, [{ tool_calls: [{ name: 'write', arguments: { path: 'orphan.txt', content: 'unsafe' } }] }])
  const result = spawnSync(executable, ['chat', '--script', f.script, '--workspace', f.workspace], {
    cwd: f.workspace, env: cliEnvironment(f), input: '/quit\n', encoding: 'utf8', timeout: 8000,
  })
  assert.equal(result.status, 0, result.stderr)
  const [session] = await f.request('listSessions')
  await f.request('selectSession', { sessionId: session.session_id })
  await f.request('setPermissionMode', { mode: 'bypass' })
  const c = connect(f)
  await c.request('deleteSession', { sessionId: session.session_id })
  const from = f.events.length
  await f.request('sendPrompt', { sessionId: session.session_id, workspace: f.workspace, model: 'fake', text: 'must not execute' })
  const error = await f.wait(e => e.event === 'run_error', from)
  assert.match(error.error, /找不到会话/)
  assert.equal(fs.existsSync(path.join(f.workspace, 'orphan.txt')), false)
  assert.deepEqual(await f.request('listSessions'), [])
})

test('protocol-rejected responses retain usage and resume past the rejected script turn', async t => {
  const f = await fixture(t, [
    { input_tokens: 10, output_tokens: 5, tool_calls: [{ id: 'used', name: 'read', arguments: { path: 'README.md' } }] },
    { input_tokens: 10, output_tokens: 5, text: 'done' },
    { input_tokens: 30, output_tokens: 20, tool_calls: [{ id: 'fresh', name: '', arguments: { path: 'rejected.txt', content: 'unsafe' } }] },
  ])
  const first = await f.run()
  const rejected = await f.run({ sessionId: first.id })
  assert.equal(rejected.result.exit_reason, 'provider_error')
  assert.equal(rejected.result.total_usage.input_tokens, 50)
  assert.equal(rejected.result.total_usage.output_tokens, 30)
  assert.equal(rejected.detail.events.filter(e => e.type === 'provider_response_rejected').length, 1)
  await f.close()
  const c = connect(f)
  await c.request('selectSession', { sessionId: first.id })
  const from = c.events.length
  await c.request('runSlash', { text: '/continue', workspace: f.workspace, sessionId: first.id })
  const done = await c.wait(e => e.event === 'run_done', from)
  assert.equal(done.result.exit_reason, 'completed')
  assert.equal(fs.existsSync(path.join(f.workspace, 'rejected.txt')), false)
})

test('BOM configuration and scripts load, and service credentials trim copy-paste whitespace', async t => {
  const f = await fixture(t, [{ text: 'BOM_LOADED' }])
  fs.mkdirSync(path.join(f.root, '.minicode'), { recursive: true })
  fs.writeFileSync(path.join(f.root, '.minicode/desktop-config.json'), '\ufeff' + JSON.stringify({ services: [], agents: [], defaultModel: 'fake' }))
  fs.writeFileSync(f.script, '\ufeff' + fs.readFileSync(f.script, 'utf8'))
  assert.deepEqual((await f.request('getConfiguration')).services, [])
  assert.equal((await f.run()).detail.messages.at(-1).content[0].text, 'BOM_LOADED')
  const server = http.createServer((req, res) => {
    assert.equal(req.url, '/v1/models')
    assert.equal(req.headers.authorization, 'Bearer fixture-key')
    res.setHeader('content-type', 'application/json')
    res.end(JSON.stringify({ data: [{ id: 'one' }] }))
  })
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
  t.after(() => new Promise(resolve => server.close(resolve)))
  const models = await f.request('fetchServiceModels', { service: { name: 'fixture', baseUrl: ` http://127.0.0.1:${server.address().port}/v1/  `, apiStyle: 'openai', apiKey: ' fixture-key\n', models: [] } })
  assert.equal(models.models[0].modelId, 'one')
  const result = spawnSync(executable, ['run', 'BOM script', '--script', f.script, '--workspace', f.workspace], { cwd: f.workspace, env: cliEnvironment(f), encoding: 'utf8', timeout: 8000 })
  assert.equal(result.status, 0, result.stderr)
  assert.match(result.stdout, /BOM_LOADED/)
})

test('publication audit rejects a signing key in an otherwise ordinary artifact', async t => {
  const f = await fixture(t)
  const root = path.join(f.root, 'publication')
  fs.mkdirSync(root)
  const key = 'audit-signing-key-fixture-only-73485'
  fs.writeFileSync(path.join(root, 'payload.txt'), key)
  const result = spawnSync(process.execPath, [path.join(projectRoot, 'release/audit.mjs'), root], {
    env: { ...cliEnvironment(f), MYGO_UPDATER_PRIVATE_KEY: key }, encoding: 'utf8', timeout: 8000,
  })
  assert.notEqual(result.status, 0)
  assert.match(result.stderr, /local credential found: payload.txt/)
  assert.ok(!result.stderr.includes(key))
})

test('TUI provider selection can use environment credentials without saved service rows', async t => {
  const f = await fixture(t)
  const server = http.createServer((req, res) => {
    assert.equal(req.url, '/v1/messages')
    assert.equal(req.headers['x-api-key'], 'env-fixture-key')
    req.resume()
    res.setHeader('content-type', 'application/json')
    res.end(JSON.stringify({ content: [{ type: 'text', text: 'ENV_PROVIDER_WORKS' }], stop_reason: 'end_turn', usage: { input_tokens: 10, output_tokens: 5 } }))
  })
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
  t.after(() => new Promise(resolve => server.close(resolve)))
  const c = connect(f, { ANTHROPIC_API_KEY: ' env-fixture-key ', ANTHROPIC_BASE_URL: `http://127.0.0.1:${server.address().port}/v1/` })
  const state = await c.request('setTuiProvider', { provider: 'anthropic', model: 'claude-audit' })
  await assert.rejects(c.request('setEffort', { effort: 'high' }), /不支持/)
  const result = await c.run({ model: state.model })
  assert.equal(result.result.exit_reason, 'completed')
  assert.equal(result.detail.messages.at(-1).content[0].text, 'ENV_PROVIDER_WORKS')
})
