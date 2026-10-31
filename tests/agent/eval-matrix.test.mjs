import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import http from 'node:http'
import {spawn} from 'node:child_process'
import {test} from 'node:test'
import {fixture, executable, projectRoot} from './helpers.mjs'
import {compareReports} from '../../evals/compare-results.mjs'

function run(args, f) {
  return new Promise((resolve, reject) => {
    const child = spawn(executable, args, {cwd:projectRoot, windowsHide:true, env:{...process.env, HOME:f.root, USERPROFILE:f.root, MINICODE_DB_PATH:f.db, MINICODE_FAKE_SCRIPT:''}})
    let stdout = '', stderr = ''
    child.stdout.on('data', s => stdout += s); child.stderr.on('data', s => stderr += s)
    child.on('error', reject); child.on('exit', code => resolve({code, stdout, stderr}))
  })
}
test('E3 two models and four independent ablations repeat with statistics and compatible resume', async t => {
  const f = await fixture(t), payloads = []
  let failing = false
  const server = http.createServer((req, res) => {
    let raw = ''; req.on('data', d => raw += d); req.on('end', () => {
      payloads.push(JSON.parse(raw)); res.setHeader('content-type', 'application/json')
      if (failing) { res.statusCode = 401; res.end('{"error":"invalid API key"}'); return }
      res.end(JSON.stringify({content:[{type:'text', text:'done'}], stop_reason:'end_turn', usage:{input_tokens:60, output_tokens:10, cache_read_input_tokens:40}}))
    })
  })
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
  t.after(() => new Promise(resolve => server.close(resolve)))
  const saved = await f.request('saveService', {service:{name:'Matrix', baseUrl:`http://127.0.0.1:${server.address().port}`, apiStyle:'anthropic', apiKey:'matrix-local-key', enabled:true, models:['model-a','model-b'].map(modelId => ({modelId, name:modelId, contextWindow:200000, maxOutputTokens:8192, supportsEffort:false, capabilities:{}}))}})
  const models = saved.services.find(s => s.name === 'Matrix').models.map(m => `${saved.services.find(s => s.name === 'Matrix').id}::${m.modelId}`)
  const tasks = path.join(f.root, 'tasks'), task = path.join(tasks, 'sample'), output = path.join(f.root, 'matrix')
  fs.mkdirSync(path.join(task, 'repo'), {recursive:true})
  fs.writeFileSync(path.join(task, 'repo', 'AGENTS.md'), 'keep-project-instruction')
  fs.writeFileSync(path.join(task, 'script.json'), JSON.stringify({turns:[{text:'done'}]}))
  fs.writeFileSync(path.join(task, 'task.yaml'), JSON.stringify({prompt:'finish', allowed_paths:[], protected_paths:[], items:[], budget:{max_rounds:2, max_seconds:10}, max_fix_attempts:0}))
  const prices = path.join(f.root, 'prices.json')
  fs.writeFileSync(prices, JSON.stringify(Object.fromEntries(models.map(m => [m,{input:1, output:2, cache_read:0.1, cache_write:1.25}]))))
  const args = ['eval','--record','--tasks-dir',tasks,'--baselines','b1','--model',models.join(','),'--ablations','full,no-compression,no-cache,no-subagents,no-prompt','--temperature','0','--prices',prices,'--output',output]
  const result = await run(args, f)
  assert.equal(result.code, 0, result.stderr)
  const records = JSON.parse(fs.readFileSync(path.join(output, 'results.json')))
  assert.equal(records.run_count, 30); assert.equal(records.passed, 30)
  assert.equal(payloads.length, 30)
  assert.equal(payloads.filter(p => typeof p.system === 'string').length, 6)
  assert.equal(payloads.filter(p => !p.tools.some(tool => tool.name === 'delegate')).length, 6)
  for (const p of payloads) { assert.equal(p.temperature, 0); assert.ok(JSON.stringify(p.system).includes('keep-project-instruction')) }
  const stats = JSON.parse(fs.readFileSync(path.join(output, 'statistics.json')))
  assert.equal(stats.groups.length, 10)
  for (const group of stats.groups) {
    assert.equal(group.runs, 3); assert.equal(group.input_tokens.mean, 100); assert.equal(group.input_tokens.variance, 0)
    assert.equal(group.cache_hit_rate, 0.4); assert.ok(Math.abs(group.cost_usd.mean - 0.000084) < 1e-10)
  }
  assert.ok(records.results.every(r => r.mechanisms && r.model && r.ablation && r.cost_usd > 0))
  for (const record of records.results) {
    assert.ok(fs.existsSync(record.provider_recording))
    const entries=fs.readFileSync(record.provider_recording,'utf8').trim().split('\n').map(JSON.parse)
    assert.ok(entries.some(e => e.kind==='provider_call' && e.session_id===record.session_id))
  }
  const resumed = await run(args, f); assert.equal(resumed.code, 0, resumed.stderr); assert.equal(payloads.length, 30)
  const invalid = await run([...args, '--repeats','1'], f); assert.notEqual(invalid.code, 0); assert.match(invalid.stderr, /at least 3/)
  assert.equal(compareReports([output,output]).rows.length, 20)
  assert.ok(compareReports([output,output]).rows.every(r => r.comparable))
  failing = true
  const failureOutput = path.join(f.root, 'failure')
  const failure = await run(['eval','--tasks-dir',tasks,'--baselines','b1','--model',models[0],'--output',failureOutput], f)
  assert.equal(failure.code, 0, failure.stderr)
  const failed = JSON.parse(fs.readFileSync(path.join(failureOutput, 'results.json')))
  assert.equal(failed.run_count, 3); assert.equal(failed.passed, 0)
  assert.ok(failed.results.every(r => r.exit_reason === 'provider_error' && r.cost_usd === null))
  const offlineOutput = path.join(f.root, 'offline')
  assert.equal((await run(['eval','--tasks-dir',tasks,'--baselines','b1','--output',offlineOutput], f)).code, 0)
  const offline = JSON.parse(fs.readFileSync(path.join(offlineOutput,'results.json')))
  assert.equal(offline.run_count, 1); assert.equal(offline.passed, 1); assert.equal(offline.results[0].cost_usd, null)
})
