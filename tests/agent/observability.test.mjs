import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import http from 'node:http'
import {spawnSync} from 'node:child_process'
import {test} from 'node:test'
import {fixture, connect, executable, projectRoot} from './helpers.mjs'

function replayFile(recording, output, f, runId) {
  const args=['replay',recording,'--output',output]; if(runId)args.push('--run-id',runId)
  const result = spawnSync(executable, args, {cwd:projectRoot, encoding:'utf8', timeout:10000, windowsHide:true,env:{...process.env,HOME:f.root,USERPROFILE:f.root,MINICODE_DB_PATH:f.db,MINICODE_PROVIDER_RECORD:'',MINICODE_PROVIDER_REPLAY:'',MINICODE_LOG_FILE:''}})
  assert.equal(result.status, 0, result.stderr || String(result.error))
  return JSON.parse(fs.readFileSync(path.join(output,'result.json'),'utf8'))
}

test('G5 a failed streamed run replays retries and tool outcomes without network or tool execution', async t => {
  const f = await fixture(t), recording = path.join(f.root, 'provider.jsonl'), log = path.join(f.root, 'events.jsonl')
  let calls = 0
  const server = http.createServer((req,res) => {
    req.resume(); req.on('end', () => {
      calls++
      if (calls === 1) { res.writeHead(429, {'retry-after':'0'}); res.end('slow down'); return }
      res.writeHead(200, {'content-type':'text/event-stream'})
      const frame = value => res.write(`data: ${JSON.stringify(value)}\n\n`)
      if (calls === 2) {
        frame({choices:[{delta:{tool_calls:[{index:0,id:'read-once',function:{name:'read',arguments:'{"path":"README.md"}'}}]},finish_reason:'tool_calls'}],usage:{prompt_tokens:10,completion_tokens:5}})
        res.end('data: [DONE]\n\n'); return
      }
      frame({choices:[{delta:{content:'partial TASK-'},finish_reason:null}]})
      frame({choices:[{delta:{content:'SECRET-VALUE'},finish_reason:null}]})
      frame({error:{type:'invalid_request_error',message:'fixture failure API_KEY=TASK-SECRET-VALUE'}})
      res.end()
    })
  })
  await new Promise(resolve => server.listen(0,'127.0.0.1',resolve))
  t.after(() => new Promise(resolve => server.close(resolve)))
  const apiKey = ['fixture','private','api','key'].join('-')
  const service = await f.request('saveService',{service:{name:'Record fixture',baseUrl:`http://127.0.0.1:${server.address().port}/v1`,apiStyle:'openai',apiKey,enabled:true,models:[{modelId:'fixture',name:'Fixture',contextWindow:200000,maxOutputTokens:8192,supportsEffort:true,capabilities:{}}]}})
  const model = service.defaultModel
  await f.close()
  const environment = {TASK_SECRET:'TASK-SECRET-VALUE',MINICODE_PROVIDER_RETRY_BASE_S:'0',MINICODE_PROVIDER_RECORD:recording,MINICODE_LOG_FILE:log}
  const live = connect(f, environment)
  await live.request('setEffort',{effort:'high'})
  const original = await live.run({model,text:'inspect API_KEY=TASK-SECRET-VALUE'})
  assert.equal(original.result.exit_reason,'provider_error'); assert.equal(calls,3, JSON.stringify(original.result))
  await live.close()
  assert.ok(fs.existsSync(recording), 'recording must survive a failed run')
  const raw = fs.readFileSync(recording,'utf8'), logs = fs.readFileSync(log,'utf8')
  assert.ok(!raw.includes('TASK-SECRET-VALUE')); assert.ok(!raw.includes(apiKey))
  assert.ok(!logs.includes('TASK-SECRET-VALUE')); assert.ok(!logs.includes(apiKey))
  const entries = raw.trim().split('\n').map(line => JSON.parse(line))
  const provider = entries.filter(e => e.kind === 'provider_call')
  assert.equal(provider.length,3)
  assert.ok(provider.every(e => e.session_id && e.run_id && e.call_id))
  assert.ok(entries.some(e => e.kind === 'tool_result'))
  const structured = logs.trim().split('\n').map(line => JSON.parse(line))
  assert.ok(structured.some(e => e.kind === 'run_start'))
  assert.ok(structured.some(e => e.kind === 'run_end' && e.data.exit_reason === 'provider_error'))
  // A changed tool source would change the third request without recorded tool outcomes.
  fs.writeFileSync(path.join(f.workspace,'README.md'),'CHANGED: do not read this during replay')
  const replay = connect(f, {...environment,MINICODE_PROVIDER_RECORD:'',MINICODE_PROVIDER_REPLAY:recording,MINICODE_LOG_FILE:path.join(f.root,'replay-events.jsonl')})
  await replay.request('setEffort',{effort:'high'})
  const reproduced = await replay.run({model,text:'inspect API_KEY=TASK-SECRET-VALUE'})
  assert.equal(reproduced.result.exit_reason, original.result.exit_reason)
  assert.equal(reproduced.result.total_usage.input_tokens, original.result.total_usage.input_tokens)
  assert.equal(calls,3, 'replay must not make any HTTP request')
  assert.ok(!JSON.stringify(reproduced.detail.messages).includes('CHANGED:'))
  assert.ok(reproduced.detail.messages.some(m => m.content.some(b => b.type === 'tool_result' && b.content.includes('repository'))))
  await replay.close()
  const mismatch = connect(f, {...environment,MINICODE_PROVIDER_RECORD:'',MINICODE_PROVIDER_REPLAY:recording,MINICODE_LOG_FILE:''})
  await mismatch.request('setEffort',{effort:'high'})
  const rejected = await mismatch.run({model,text:'a different request'})
  assert.notEqual(rejected.result.exit_reason, 'completed')
  assert.match(rejected.result.error, /replay.*match/i)
  assert.equal(calls,3)
  await mismatch.close()
  const standalone = replayFile(recording,path.join(f.root,'nested','standalone'),f)
  assert.equal(standalone.exit_reason,original.result.exit_reason)
  assert.equal(standalone.total_usage.input_tokens,original.result.total_usage.input_tokens)
  assert.equal(calls,3)
})

test('G5 replay restores failed acceptance without executing its command', async t => {
  const f=await fixture(t,[{text:'finish'},{text:'finish again'}]), recording=path.join(f.root,'goal.jsonl')
  await f.request('getState')
  await f.close()
  const live=connect(f,{MINICODE_PROVIDER_RECORD:recording})
  const acceptance=path.join(f.root,'acceptance.json')
  fs.writeFileSync(acceptance,JSON.stringify({max_fix_attempts:2,items:[{id:'fails',type:'command',command:'node -e "require(\'node:fs\').writeFileSync(\'touched\',\'yes\');process.exit(1)"'}]}))
  await live.request('setPermissionMode',{mode:'bypass'})
  await live.request('setAcceptance',{path:acceptance})
  const first=await live.run()
  assert.equal(first.result.exit_reason,'goal_not_met')
  const original=await live.run({text:'a new task resets acceptance attempts'})
  assert.equal(original.result.exit_reason,'goal_not_met')
  assert.ok(fs.existsSync(path.join(f.workspace,'touched')))
  await live.close()
  const output=path.join(f.root,'goal-replay'), result=replayFile(recording,output,f)
  assert.equal(result.exit_reason,'goal_not_met')
  assert.deepEqual(result.total_usage,original.result.total_usage)
  assert.equal(fs.existsSync(path.join(output,'workspace','touched')),false)
})

test('G5 replay includes usage consumed by a delegate', async t => {
  const f=await fixture(t,[{tool_calls:[{name:'delegate',arguments:{kind:'explore',task:'inspect'}}]},{text:JSON.stringify({summary:'done',findings:[],evidence_refs:[],unresolved:[]})},{error:'provider HTTP 401 fixture failure'}])
  await f.request('getState'); await f.close()
  const recording=path.join(f.root,'delegate.jsonl'), live=connect(f,{MINICODE_PROVIDER_RECORD:recording})
  await live.request('setPermissionMode',{mode:'bypass'})
  const original=await live.run()
  assert.equal(original.result.exit_reason,'provider_error')
  assert.equal(original.result.total_usage.input_tokens,200)
  await live.close()
  const result=replayFile(recording,path.join(f.root,'delegate-replay'),f)
  assert.equal(result.exit_reason,'provider_error')
  assert.deepEqual(result.total_usage,original.result.total_usage)
})

test('G5 replay preserves a provider deadline without waiting for the network', async t => {
  const f=await fixture(t), server=http.createServer(req => req.resume())
  await new Promise(resolve => server.listen(0,'127.0.0.1',resolve))
  t.after(() => {server.closeAllConnections();return new Promise(resolve => server.close(resolve))})
  const saved=await f.request('saveService',{service:{name:'Deadline',baseUrl:`http://127.0.0.1:${server.address().port}/v1`,apiStyle:'openai',apiKey:'local-fixture-key',enabled:true,models:[{modelId:'fixture',name:'Fixture',contextWindow:200000,maxOutputTokens:8192,capabilities:{}}]}})
  await f.close()
  const recording=path.join(f.root,'deadline.jsonl'), live=connect(f,{MINICODE_PROVIDER_RECORD:recording})
  await live.request('setBudget',{max_seconds:3})
  const original=await live.run({model:saved.defaultModel})
  assert.equal(original.result.exit_reason,'time_budget')
  await live.close()
  const entries=fs.readFileSync(recording,'utf8').trim().split('\n').map(JSON.parse)
  assert.ok(entries.some(e => e.kind==='provider_call' && e.failure.kind==='deadline'))
  assert.equal(replayFile(recording,path.join(f.root,'deadline-replay'),f).exit_reason,'time_budget')
})

test('G5 an appended recording can select an earlier failed run', async t => {
  const f=await fixture(t,[{error:'provider HTTP 401 failure'},{text:'next task completed'}])
  await f.request('getState'); await f.close()
  const recording=path.join(f.root,'multiple.jsonl'), live=connect(f,{MINICODE_PROVIDER_RECORD:recording})
  assert.equal((await live.run({text:'first'})).result.exit_reason,'provider_error')
  assert.equal((await live.run({text:'second'})).result.exit_reason,'completed')
  await live.close()
  const entries=fs.readFileSync(recording,'utf8').trim().split('\n').map(JSON.parse)
  const first=entries.find(e => e.kind==='run_start')
  assert.equal(replayFile(recording,path.join(f.root,'latest'),f).exit_reason,'completed')
  assert.equal(replayFile(recording,path.join(f.root,'earlier'),f,first.run_id).exit_reason,'provider_error')
})

test('G5 an unwritable recording leaves the runtime idle and executes no tools', async t => {
  const f=await fixture(t,[{tool_calls:[{name:'write',arguments:{path:'should-not-exist',content:'no'}}]}])
  await f.request('getState'); await f.close()
  const live=connect(f,{MINICODE_PROVIDER_RECORD:f.root})
  await live.settle()
  assert.equal((await live.request('getState')).running,false)
  assert.equal(fs.existsSync(path.join(f.workspace,'should-not-exist')),false)
})

test('G5 a provider panic keeps its failure when replayed', async t => {
  const f=await fixture(t,[{panic:'recorded fixture panic'}])
  await f.request('getState'); await f.close()
  const recording=path.join(f.root,'panic.jsonl'), live=connect(f,{MINICODE_PROVIDER_RECORD:recording,MINICODE_PROVIDER_RETRY_BASE_S:'0'})
  const original=await live.settle()
  assert.equal(original.done.event,'run_error')
  assert.match(original.done.error,/fake provider panic: recorded fixture panic/)
  await live.close()
  const result=spawnSync(executable,['replay',recording,'--output',path.join(f.root,'panic-replay')],{cwd:projectRoot,encoding:'utf8',timeout:10000,windowsHide:true,env:{...process.env,HOME:f.root,USERPROFILE:f.root,MINICODE_DB_PATH:f.db,MINICODE_PROVIDER_RETRY_BASE_S:'0'}})
  assert.equal(result.status,1,result.stdout)
  assert.match(result.stderr,/fake provider panic: recorded fixture panic/)
})
