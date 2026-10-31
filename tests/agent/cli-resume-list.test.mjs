import assert from 'node:assert/strict'
import test from 'node:test'
import {spawnSync} from 'node:child_process'
import {fixture,executable} from './helpers.mjs'
test('L13 CLI resume without arguments lists recent sessions instead of resolving an empty prefix',async t=>{
 const f=await fixture(t,[{text:'done'}]);const saved=await f.run()
 const result=spawnSync(executable,['chat','--provider','fake','--workspace',f.workspace],{env:{...process.env,HOME:f.root,USERPROFILE:f.root,MINICODE_DB_PATH:f.db},input:'/resume\n/quit\n',encoding:'utf8',timeout:12000})
 assert.equal(result.status,0,result.stderr)
 assert.match(result.stdout,new RegExp(saved.id))
 assert.doesNotMatch(result.stderr,/非空会话|不唯一或不存在/)
})
