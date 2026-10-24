import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import {fileURLToPath} from 'node:url'
import {spawn} from 'node:child_process'
import {setTimeout as delay} from 'node:timers/promises'
export const projectRoot=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'../..')
export const executable=process.env.MINICODE_AGENT||path.join(projectRoot,'target',fs.existsSync(path.join(projectRoot,'target/release',process.platform==='win32'?'minicode.exe':'minicode'))?'release':'debug',process.platform==='win32'?'minicode.exe':'minicode')
export function profile(turns=[]){
  const root=fs.mkdtempSync(path.join(os.tmpdir(),'minicode-rust-test-')),workspace=path.join(root,'repo')
  fs.mkdirSync(workspace);fs.writeFileSync(path.join(workspace,'README.md'),'测试 repository\nsecond line\n')
  const script=path.join(root,'script.json');fs.writeFileSync(script,JSON.stringify({turns}))
  return {root,workspace,script,db:path.join(root,'sessions.db'),connections:new Set()}
}
export function connect(fixture,extraEnv={}){
  const env={...process.env,HOME:fixture.root,USERPROFILE:fixture.root,MINICODE_DB_PATH:fixture.db,MINICODE_FAKE_SCRIPT:fixture.script,ANTHROPIC_API_KEY:'',COMMANDCODE_API_KEY:'',MINICODE_RESOURCES:'',...extraEnv}
  const child=spawn(executable,['bridge'],{cwd:projectRoot,env,stdio:['pipe','pipe','pipe'],windowsHide:true})
  const events=[],pending=new Map();let seq=0,output='',errors='',closed=false
  child.stdout.setEncoding('utf8');child.stderr.setEncoding('utf8');child.stderr.on('data',s=>{errors+=s})
  child.stdout.on('data',s=>{output+=s;while(output.includes('\n')){const i=output.indexOf('\n'),line=output.slice(0,i);output=output.slice(i+1);if(!line)continue;const value=JSON.parse(line);if(value.id!==undefined){const item=pending.get(value.id);if(item){pending.delete(value.id);clearTimeout(item.timer);if(value.error)item.reject(Error(value.error));else item.resolve(value.result)}}else events.push(value)}})
  child.on('error',error=>{for(const item of pending.values())item.reject(error);pending.clear()})
  child.on('exit',code=>{for(const item of pending.values()){clearTimeout(item.timer);item.reject(Error(`agent exited ${code}: ${errors}`))}pending.clear()})
  function request(method,params={}){const id=++seq;return new Promise((resolve,reject)=>{const timer=setTimeout(()=>{pending.delete(id);reject(Error(`${method} timed out: ${errors}`))},15000);pending.set(id,{resolve,reject,timer});child.stdin.write(JSON.stringify({id,method,params})+'\n')})}
  async function wait(check,from=0){for(let i=0;i<1500;i++){const value=events.slice(from).find(check);if(value)return value;await delay(10)}throw Error(`event timeout: ${errors}\n${JSON.stringify(events.slice(from))}`)}
  async function run(params={}){const from=events.length;const started=await request('sendPrompt',{text:'检查并验证',workspace:fixture.workspace,model:'fake',...params});const done=await wait(e=>(e.event==='run_done'||e.event==='run_error')&&e.sessionId===started.sessionId,from);if(done.event==='run_error')throw Error(done.error);return {id:started.sessionId,result:done.result,events:events.slice(from),detail:await request('getSession',{sessionId:started.sessionId})}}
  // Like run, but resolves with the terminal run_done or run_error event instead of throwing.
  async function settle(params={}){const from=events.length;const started=await request('sendPrompt',{text:'检查并验证',workspace:fixture.workspace,model:'fake',...params});const done=await wait(e=>(e.event==='run_done'||e.event==='run_error')&&e.sessionId===started.sessionId,from);return {id:started.sessionId,done,events:events.slice(from)}}
  async function close(){if(closed)return;closed=true;const exited=new Promise(resolve=>{if(child.exitCode!==null)resolve();else child.once('exit',resolve)});child.stdin.end();await Promise.race([exited,delay(5000)]);if(child.exitCode===null){if(process.platform==='win32')await new Promise(resolve=>spawn('taskkill',['/PID',String(child.pid),'/T','/F'],{stdio:'ignore',windowsHide:true}).once('exit',resolve));else child.kill('SIGKILL');await exited}}
  const api={child,events,request,run,settle,wait,close,get stderr(){return errors}};fixture.connections?.add(api);return api
}
// Windows can briefly retain directory handles after a cancelled job exits.
// Bound retries to transient cleanup errors; persistent process leaks still fail.
export async function fixture(t,turns=[]){const f=profile(turns),c=connect(f);t.after(async()=>{for(const connection of f.connections)await connection.close();await c.close();if(path.dirname(f.root)!==path.resolve(os.tmpdir()))throw Error('unsafe test cleanup');fs.rmSync(f.root,{recursive:true,force:true,maxRetries:10,retryDelay:100})});return {...f,...c}}
// Asserts that events contain matches for every predicate, in order (other events may interleave).
export function expectSequence(events,predicates){let at=0;for(const [n,check] of predicates.entries()){const found=events.slice(at).findIndex(check);if(found<0)throw Error(`sequence step ${n} missing after index ${at}: ${JSON.stringify(events.map(e=>e.event==='agent_event'?e.item.type:e.event))}`);at+=found+1}}
export function pidAlive(pid){try{process.kill(pid,0);return true}catch(error){return error.code==='EPERM'}}
export async function until(check,ms=10000){const end=Date.now()+ms;while(Date.now()<end){if(await check())return true;await delay(25)}return false}
