import assert from 'node:assert/strict'
import test from 'node:test'
import {spawnSync} from 'node:child_process'
import fs from 'node:fs'
import path from 'node:path'
import {fixture,executable} from './helpers.mjs'
test('L7 CLI defaults to bounded output and verbose preserves the full result', async t=>{
 const f=await fixture(t,[{tool_calls:[{name:'read',arguments:{path:'long.txt'}}]},{text:'done'}])
 await f.request('listSessions')
 fs.writeFileSync(path.join(f.workspace,'long.txt'), Array.from({length:300},(_,i)=>`ROW_${i}`).join('\n'))
 const options={cwd:f.workspace,env:{...process.env,HOME:f.root,USERPROFILE:f.root,MINICODE_DB_PATH:f.db,MINICODE_FAKE_SCRIPT:f.script},encoding:'utf8',timeout:15000}
 const args=['run','inspect','--script',f.script,'--workspace',f.workspace]
 const normal=spawnSync(executable,args,options),verbose=spawnSync(executable,[...args,'-v'],options)
 assert.equal(normal.status,0,normal.stderr);assert.equal(verbose.status,0,verbose.stderr)
 assert.ok(!normal.stdout.includes('ROW_299'));assert.match(verbose.stdout,/ROW_299/)
})
