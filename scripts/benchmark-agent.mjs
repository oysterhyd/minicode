import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import {spawn,spawnSync} from 'node:child_process'
import {performance} from 'node:perf_hooks'
import {DatabaseSync} from 'node:sqlite'
const options=Object.fromEntries(process.argv.slice(2).map((value,i,a)=>value.startsWith('--')?[value.slice(2),a[i+1]]:[]).filter(a=>a.length))
const agent=path.resolve(options.agent||'target/release/minicode.exe'),legacy=options.legacy&&path.resolve(options.legacy),baseline=options.baseline&&path.resolve(options.baseline),samples=Number(options.samples||7)
function median(v){const sorted=[...v].sort((a,b)=>a-b);return sorted[Math.floor(sorted.length/2)]}
async function memory(pid){if(process.platform!=='win32')return {};const result=spawnSync('pwsh',['-NoProfile','-Command',`$p=Get-Process -Id ${pid};@{working_set=$p.WorkingSet64;private_bytes=$p.PrivateMemorySize64;peak_working_set=$p.PeakWorkingSet64;cpu_s=$p.TotalProcessorTime.TotalSeconds}|ConvertTo-Json -Compress`],{encoding:'utf8',windowsHide:true});if(result.status!==0)throw Error('Agent memory probe failed');return JSON.parse(result.stdout)}
async function measure(kind,backend){
  const root=fs.mkdtempSync(path.join(os.tmpdir(),'minicode-agent-bench-')),workspace=path.join(root,'repo');fs.mkdirSync(workspace);fs.writeFileSync(path.join(workspace,'data.txt'),'benchmark data\n'.repeat(100))
  const script=path.join(root,'script.json');const turns=Array.from({length:24},(_,i)=>({tool_calls:[{name:'read',arguments:{path:'data.txt',offset:i*4+1,limit:1}},{name:'read',arguments:{path:'data.txt',offset:i*4+2,limit:1}}]}));turns.push({text:'completed'});fs.writeFileSync(script,JSON.stringify({turns}))
  const env={...process.env,HOME:root,USERPROFILE:root,MINICODE_DB_PATH:path.join(root,'session.db'),MINICODE_FAKE_SCRIPT:script,ANTHROPIC_API_KEY:'',COMMANDCODE_API_KEY:''}
  const file=backend==='python'?path.join(legacy,'python/python.exe'):backend==='before'?baseline:agent,args=backend==='python'?['-I','-X','utf8','-u',path.join(legacy,'bridge.py')]:['bridge']
  const started=performance.now(),child=spawn(file,args,{env,stdio:['pipe','pipe','pipe'],windowsHide:true}),pending=new Map(),events=[],fragments=[];let seq=0,errors=''
  // Scan each incoming chunk once. Repeatedly scanning a growing 20 MB string
  // would benchmark the harness's quadratic NDJSON assembly instead of the agent.
  child.stdout.setEncoding('utf8');child.stderr.setEncoding('utf8');child.stderr.on('data',s=>errors+=s);child.stdout.on('data',s=>{let start=0,end;while((end=s.indexOf('\n',start))>=0){fragments.push(s.slice(start,end));const line=fragments.join('');fragments.length=0;start=end+1;const v=JSON.parse(line);if(v.id!==undefined){const p=pending.get(v.id);if(p){pending.delete(v.id);v.error?p.reject(Error(v.error)):p.resolve(v.result)}}else events.push(v)}if(start<s.length)fragments.push(s.slice(start))})
  const request=(method,params={})=>new Promise((resolve,reject)=>{const id=++seq;pending.set(id,{resolve,reject});child.stdin.write(JSON.stringify({id,method,params})+'\n')})
  const initialized=await request('getState'),cold_ms=performance.now()-started;let duration_ms=cold_ms
  const idle=await memory(child.pid)
  if(kind==='turns'){await request('setPermissionMode',{mode:'bypass'});const clock=performance.now();const result=await request('sendPrompt',{model:'fake',workspace,text:'Read the fixture repeatedly.'});for(let i=0;i<15000&&!events.some(e=>e.event==='run_done'&&e.sessionId===result.sessionId);i++)await new Promise(resolve=>setTimeout(resolve,1));const done=events.find(e=>e.event==='run_done'&&e.sessionId===result.sessionId);if(done?.result?.exit_reason!=='completed')throw Error('Agent did not complete benchmark');duration_ms=performance.now()-clock;}
  if(kind==='history'||kind==='poll'||kind==='stats'){
    const db=new DatabaseSync(env.MINICODE_DB_PATH);const id='abcdef0123456789abcdef0123456789';db.prepare('INSERT INTO sessions(session_id,created_at,workspace,provider,model,status,rounds,input_tokens,output_tokens) VALUES(?,?,?,?,?,?,?,?,?)').run(id,'2026-01-01T00:00:00+00:00',workspace,'fake','fake','completed',0,0,0);const insert=db.prepare('INSERT INTO messages VALUES(?,?,?,?)');db.exec('BEGIN');for(let i=0;i<2000;i++)insert.run(id,i,i%2?'assistant':'user',JSON.stringify([{type:'text',text:'x'.repeat(10000)}]));db.exec('COMMIT');db.close();
    if(kind==='stats'){
      const db=new DatabaseSync(env.MINICODE_DB_PATH),insert=db.prepare('INSERT INTO events VALUES(?,?,?,?,?)');db.exec('BEGIN');for(let i=0;i<2000;i++)insert.run(id,i,i%2?'assistant_message':'tool_call_result','2026-01-01T00:00:00+00:00',JSON.stringify(i%2?{text:'x'.repeat(5000),reasoning:'x'.repeat(5000),usage:{output_tokens:1,available:true},request_seconds:0.1,generation_seconds:0.05}:{output_detail:'x'.repeat(10000)}));db.exec('COMMIT');db.close();
      const clock=performance.now(),result=await request('getState',{sessionId:id});duration_ms=performance.now()-clock;if(result.statistics.requests!==1000)throw Error('statistics benchmark lost responses');
    }
    else if(kind==='history'){const clock=performance.now();const result=await request('getSession',{sessionId:id});duration_ms=performance.now()-clock;if(result.messages.length!==2000)throw Error('history benchmark lost messages');}
    else {await request('getState',{sessionId:id});const values=[];for(let i=0;i<40;i++){const clock=performance.now();await request('getState',{sessionId:id});values.push(performance.now()-clock)}duration_ms=median(values);}
  }
  const used=await memory(child.pid);const exited=new Promise(resolve=>child.once('exit',resolve));child.stdin.end();await exited;if(errors.includes('Traceback'))throw Error(errors);if(path.dirname(root)!==path.resolve(os.tmpdir()))throw Error('Unsafe benchmark cleanup');fs.rmSync(root,{recursive:true,force:true});return {duration_ms,cold_ms,idle,...used}
}
const record={date:new Date().toISOString(),platform:process.platform,architecture:process.arch,node:process.version,samples,agent,legacy,baseline,measurements:{}}
for(const kind of (options.kinds||'cold,turns,history,poll').split(',')) {
  const backends=baseline?['before','after']:legacy?['python','rust']:['rust'],runs=Object.fromEntries(backends.map(b=>[b,[]]));record.measurements[kind]={}
  // Reverse each pair so neither implementation always benefits from order.
  for(let i=0;i<samples;i++)for(const backend of i%2?[...backends].reverse():backends)runs[backend].push(await measure(kind,backend))
  for(const backend of backends){const values=runs[backend],numeric=Object.keys(values[0]).filter(k=>typeof values[0][k]==='number');const medians=Object.fromEntries(numeric.map(k=>[k,median(values.map(r=>r[k]))]));medians.idle_working_set=median(values.map(r=>r.idle.working_set));medians.idle_private_bytes=median(values.map(r=>r.idle.private_bytes));record.measurements[kind][backend]={medians,runs:values};console.log(kind+' '+backend+': '+JSON.stringify(medians))}
}
fs.mkdirSync(path.dirname(options.output||'output/rust-migration/benchmark.json'),{recursive:true});fs.writeFileSync(options.output||'output/rust-migration/benchmark.json',JSON.stringify(record,null,2)+'\n')
