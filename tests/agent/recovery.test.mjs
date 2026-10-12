import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import {test} from 'node:test'
import {DatabaseSync} from 'node:sqlite'
import {createHash} from 'node:crypto'
import {fixture} from './helpers.mjs'
const id='1234567890abcdef1234567890abcdef'
function legacyScriptIdentity(script){return createHash('sha256').update(JSON.stringify({turns:script.turns.map(t=>({text:t.text??null,tool_calls:(t.tool_calls??[]).map(c=>({name:c.name,arguments:c.arguments??{},id:c.id??null})),input_tokens:t.input_tokens??null,output_tokens:t.output_tokens??null,cache_read_tokens:t.cache_read_tokens??null,cache_write_tokens:t.cache_write_tokens??null,stop_reason:t.stop_reason??null})),exhausted_text:script.exhausted_text??'(fake provider: script exhausted)',default_input_tokens:script.default_input_tokens??100,default_output_tokens:script.default_output_tokens??20})).digest('hex')}
function legacy(f,{journal=false,future=false}={}){
  for (const suffix of ['', '-wal', '-shm']) fs.rmSync(f.db + suffix, {force:true})
  // This v3 layout comes from the released schema, before cache columns.
  // It exercises a real on-disk upgrade rather than a newly created Rust DB.
  const db=new DatabaseSync(f.db)
  db.exec(`CREATE TABLE sessions(session_id TEXT PRIMARY KEY,created_at TEXT NOT NULL,workspace TEXT NOT NULL,provider TEXT NOT NULL,model TEXT NOT NULL,status TEXT NOT NULL,exit_reason TEXT,rounds INTEGER NOT NULL,input_tokens INTEGER NOT NULL,output_tokens INTEGER NOT NULL);
  CREATE TABLE messages(session_id TEXT NOT NULL,seq INTEGER NOT NULL,role TEXT NOT NULL,content TEXT NOT NULL,PRIMARY KEY(session_id,seq));
  CREATE TABLE events(session_id TEXT NOT NULL,seq INTEGER NOT NULL,type TEXT NOT NULL,timestamp TEXT NOT NULL,data TEXT NOT NULL,PRIMARY KEY(session_id,seq));
  PRAGMA user_version=${future?8:3};`)
  db.prepare('INSERT INTO sessions VALUES(?,?,?,?,?,?,?,?,?,?)').run(id,'2026-01-01T00:00:00+00:00',f.workspace,'fake','fake','paused','cancelled',1,100,20)
  db.prepare('INSERT INTO messages VALUES(?,?,?,?)').run(id,0,'user',JSON.stringify([{type:'text',text:'legacy task'}]))
  db.prepare('INSERT INTO messages VALUES(?,?,?,?)').run(id,1,'assistant',JSON.stringify([{type:'tool_use',id:'fake_tool_1',name:'write',input:{path:'legacy-write.txt',content:'once'}}]))
  db.prepare('INSERT INTO events VALUES(?,?,?,?,?)').run(id,0,'session_start','2026-01-01T00:00:00+00:00',JSON.stringify({provider_resume_identity:legacyScriptIdentity(JSON.parse(fs.readFileSync(f.script,'utf8'))),budget:{max_rounds:0,max_total_tokens:777,max_seconds:0}}))
  db.prepare('INSERT INTO events VALUES(?,?,?,?,?)').run(id,1,'assistant_message','2026-01-01T00:00:00+00:00',JSON.stringify({text:'',usage:{input_tokens:100,output_tokens:20},tool_call_ids:['fake_tool_1']}))
  db.prepare('INSERT INTO events VALUES(?,?,?,?,?)').run(id,2,'tool_call_start','2026-01-01T00:00:00+00:00',JSON.stringify({call_id:'fake_tool_1',name:'write',arguments:{path:'legacy-write.txt',content:'once'}}))
  if(journal){db.exec('CREATE TABLE tool_results(session_id TEXT NOT NULL,call_id TEXT NOT NULL,result TEXT NOT NULL,PRIMARY KEY(session_id,call_id))');db.prepare('INSERT INTO tool_results VALUES(?,?,?)').run(id,'fake_tool_1',JSON.stringify({type:'tool_result',tool_use_id:'fake_tool_1',content:'already completed',is_error:false,images:[]}))}
  db.close()
}
test('released SQLite v3 database upgrades and unknown side effects are not replayed',async t=>{const f=await fixture(t,[{tool_calls:[{name:'write',arguments:{path:'legacy-write.txt',content:'once'}}]},{text:'legacy resumed safely'}]);await f.close();legacy(f);const {connect}=await import('./helpers.mjs');const c=connect(f);const state=await c.request('getState',{sessionId:id});assert.equal(state.taskPending,true);assert.equal(state.budget.max_total_tokens,777);const from=c.events.length;await c.request('runSlash',{text:'/continue',workspace:f.workspace,sessionId:id});const done=await c.wait(e=>e.event==='run_done',from);assert.equal(done.result.exit_reason,'completed');assert.equal(fs.existsSync(path.join(f.workspace,'legacy-write.txt')),false);const detail=await c.request('getSession',{sessionId:id});assert.ok(detail.events.some(e=>e.type==='side_effect_unknown'));assert.ok(detail.messages.some(m=>m.content.some(b=>b.text==='legacy resumed safely')));})
test('durable tool-result journal repairs missing delivery without another execution',async t=>{const f=await fixture(t,[{tool_calls:[{name:'write',arguments:{path:'legacy-write.txt',content:'once'}}]},{text:'done'}]);await f.close();legacy(f,{journal:true});const {connect}=await import('./helpers.mjs');const c=connect(f);await c.request('getState',{sessionId:id});await c.request('runSlash',{text:'/continue',workspace:f.workspace,sessionId:id});await c.wait(e=>e.event==='run_done');const detail=await c.request('getSession',{sessionId:id});assert.equal(fs.existsSync(path.join(f.workspace,'legacy-write.txt')),false);assert.ok(detail.messages.some(m=>m.content.some(b=>b.type==='tool_result'&&b.content==='already completed')));assert.ok(!detail.events.some(e=>e.type==='side_effect_unknown'));})
test('newer schema is rejected without modifying the database',async t=>{const f=await fixture(t);await f.close();legacy(f,{future:true});const before=fs.readFileSync(f.db);const {spawnSync}=await import('node:child_process');const {executable}=await import('./helpers.mjs');const result=spawnSync(executable,['--db',f.db,'sessions','list'],{encoding:'utf8'});assert.notEqual(result.status,0);assert.match(result.stderr,/newer version/);assert.deepEqual(fs.readFileSync(f.db),before);})
