import assert from 'node:assert/strict'
import test from 'node:test'
import {execFileSync} from 'node:child_process'
import {fixture} from './helpers.mjs'
test('TU8 shared state supplies the actual Git branch before and after a run',async t=>{
 const f=await fixture(t,[{text:'done'}])
 execFileSync('git',['init','-q'],{cwd:f.workspace,windowsHide:true})
 const branch=execFileSync('git',['symbolic-ref','--short','HEAD'],{cwd:f.workspace,encoding:'utf8',windowsHide:true}).trim()
 const initial=await f.request('initialize',{workspace:f.workspace})
 assert.equal(initial.state.branch,branch)
 const run=await f.run()
 assert.equal((await f.request('getState',{workspace:f.workspace})).branch,branch)
 await f.request('resetSession')
 assert.equal((await f.request('getState')).branch,branch)
 assert.equal((await f.request('selectSession',{sessionId:run.id,deferRuntime:true})).branch,branch)
 await f.request('runSlash',{workspace:f.workspace,text:'/clear'})
 assert.equal((await f.request('getState')).branch,branch)
})
